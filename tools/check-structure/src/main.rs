//! Architectural linter for Rust workspaces: enforces deep modules and narrow
//! interfaces, working like `cargo check` / `cargo clippy`.

mod allow;
mod configuration;
mod format;
mod graph;
mod indexing;
mod lint;
mod model;
mod progress;
mod scip;
mod source;
mod workspace;

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use allow::AllowList;
use configuration::Configuration;
use model::Model;
use workspace::Workspace;

pub const TOOL: &str = env!("CARGO_BIN_NAME");

#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

struct Options {
    scope: Option<String>,
    allow: Option<PathBuf>,
    update_allow: Option<PathBuf>,
    ratchet: bool,
    timings: bool,
    all_ties: bool,
    configuration: Configuration,
}

fn usage() -> String {
    format!(
        "\
usage: {TOOL} [crate_or_mod] [--allow <path> [--ratchet]] [--update-allow <path>]
       [--timings] [--all-ties] [--target <triple>] [-F, --features <features>] [--all-features]
       [--no-default-features]

Runs architectural lints (module-cycle, tangle, field-leak, shallow-module,
wide-directory) on the Cargo workspace. Under GitHub Actions each error is also
written as a workflow annotation.

options:
  <crate_or_mod>               check only one crate or module (e.g. my-crate)
  --allow <path>               suppress listed existing debt; new errors still fail
  --ratchet                    with --allow, fail while the list holds an entry
                               that is fixed or smaller (a circle that shrank),
                               so the debt only shrinks; not with <crate_or_mod>
  --update-allow <path>        write current diagnostics to <path>; not with
                               <crate_or_mod>, which would drop the rest
  --timings                    print on stderr how long the index, the model and the
                               lints took
  --all-ties                   list every tie of a circle, not the first seven

configuration, as `cargo check --workspace` takes it:
  --target <triple>            check for the target triple, not the host's
  -F, --features <features>    space or comma separated list of features to
                               activate; may be given more than once
  --all-features               activate all features of every member
  --no-default-features        do not activate the `default` feature

A run checks one configuration: the host's target with default features,
unless these flags or cargo's own configuration (`build.target`, `RUSTFLAGS`)
choose another. Code behind a `cfg` that it leaves off, such as a
`#[cfg(windows)]` module on Linux, is not checked: run once for each
configuration that matters.

A flag's value is the next word, or follows `=`: `--target=<triple>`.
"
    )
}

/// The flags, and whether each takes a value. `-F` is `--features`.
const FLAGS: [(&str, bool); 9] = [
    ("--allow", true),
    ("--ratchet", false),
    ("--timings", false),
    ("--all-ties", false),
    ("--update-allow", true),
    ("--target", true),
    ("--features", true),
    ("--all-features", false),
    ("--no-default-features", false),
];

/// Reads the command line as cargo reads its own: a flag's value is the next
/// word, or follows `=` in the flag's own (`--target=<triple>`, `-F=a,b`) or
/// `-F` (`-Fa,b`); only `--target` and `--features` may be given more than
/// once.
fn parse_arguments(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        scope: None,
        allow: None,
        update_allow: None,
        ratchet: false,
        timings: false,
        all_ties: false,
        configuration: Configuration::default(),
    };
    let mut given = Vec::new();
    let mut words = arguments.iter().map(String::as_str).peekable();
    while let Some(word) = words.next() {
        if !word.starts_with('-') {
            if options.scope.replace(word.to_owned()).is_some() {
                return Err(format!("unexpected argument '{word}'"));
            }
            continue;
        }
        let (flag, attached) = split_flag(word);
        let Some(&(flag, takes_value)) = FLAGS.iter().find(|(name, _)| *name == flag) else {
            return Err(format!("unexpected argument '{word}'"));
        };
        if given.contains(&flag) && !matches!(flag, "--target" | "--features") {
            return Err(format!("'{flag}' cannot be given more than once"));
        }
        given.push(flag);
        let value = match (takes_value, attached) {
            (true, Some(value)) => value,
            (true, None) => words
                .next_if(|next| !next.starts_with('-'))
                .ok_or_else(|| format!("'{flag}' needs a value"))?,
            (false, Some(value)) => return Err(format!("'{flag}' takes no value: '{value}'")),
            (false, None) => "",
        };
        let configuration = &mut options.configuration;
        match flag {
            "--allow" => options.allow = Some(PathBuf::from(value)),
            "--update-allow" => options.update_allow = Some(PathBuf::from(value)),
            "--ratchet" => options.ratchet = true,
            "--timings" => options.timings = true,
            "--all-ties" => options.all_ties = true,
            "--target" => configuration.target(value)?,
            "--features" => configuration.features(value),
            "--all-features" => configuration.all_features(),
            "--no-default-features" => configuration.no_default_features(),
            _ => unreachable!("{flag} has no meaning"),
        }
    }
    if options.allow.is_some() && options.update_allow.is_some() {
        return Err("'--allow' and '--update-allow' cannot be given together".to_owned());
    }
    if options.ratchet && options.allow.is_none() {
        return Err("'--ratchet' needs '--allow'".to_owned());
    }

    // A list is of the whole workspace: written from a scope it would drop
    // every entry outside it, and judged by a scope it would call them fixed.
    if options.scope.is_some() && (options.ratchet || options.update_allow.is_some()) {
        return Err(
            "'--ratchet' and '--update-allow' judge the whole list, so they cannot be given a crate or module"
                .to_owned(),
        );
    }
    Ok(options)
}

/// The flag `word` names, and the value written in it: `--target=<triple>`,
/// and `-F<features>` or `-F=<features>`.
fn split_flag(word: &str) -> (&str, Option<&str>) {
    if let Some(value) = word.strip_prefix("-F") {
        let value = (!value.is_empty()).then(|| value.strip_prefix('=').unwrap_or(value));
        return ("--features", value);
    }
    match word.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (word, None),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        arguments.as_slice(),
        [help] if help == "-h" || help == "--help" || help == "help"
    ) {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }
    let options = match parse_arguments(&arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{TOOL}: {message}");
            eprint!("{}", usage());
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{TOOL}: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(options: Options) -> Result<bool, Error> {
    let started = Instant::now();
    let workspace = Workspace::discover()?;
    let (model, indexed, modelled) = load_model(&workspace, &options.configuration)?;
    let model_ready = Instant::now();
    let scope = match &options.scope {
        Some(query) => Some(
            lint::resolve_node(&model, query)
                .ok_or_else(|| Error::new(format!("{query}: no such crate or module")))?,
        ),
        None => None,
    };
    let all = lint::check(&model, scope);
    if options.timings {
        eprintln!(
            "{TOOL}: timings: index {:.1?}, model {:.1?}, lints {:.1?}, total {:.1?}",
            indexed,
            modelled,
            model_ready.elapsed(),
            started.elapsed()
        );
    }

    if let Some(path) = options.update_allow {
        let allow = AllowList::from_diagnostics(&all);
        allow.write(&path)?;
        eprintln!("{TOOL}: wrote {} entries to {}", all.len(), path.display());
        return Ok(true);
    }

    let (reported, fixed_allowed, allow_shown) = match &options.allow {
        Some(path) => {
            let allow = AllowList::read(path)?;

            // Under a scope the findings are only some of the workspace's, so
            // an entry the scope does not reach is not one that was fixed.
            let fixed = if scope.is_some() {
                0
            } else {
                allow.stale_count(&all)
            };
            let active: Vec<_> = all.into_iter().filter(|d| !allow.is_allowed(d)).collect();
            (active, fixed, Some(path.display().to_string()))
        }
        None => (all, 0, None),
    };

    print!(
        "{}",
        format::render(
            &reported,
            fixed_allowed,
            allow_shown.as_deref(),
            options.all_ties
        )
    );
    let tight = !options.ratchet || fixed_allowed == 0;
    if !tight && let Some(path) = &options.allow {
        eprintln!(
            "{TOOL}: {fixed_allowed} allowed entries in {} are fixed or smaller; \
             run `{TOOL} --update-allow {}` and commit it, so the debt cannot grow back",
            path.display(),
            path.display()
        );
    }
    Ok(reported.is_empty() && tight)
}

/// The model, and how long its two steps took: the index (read from the cache
/// or built by rust-analyzer) and the model made from it.
fn load_model(
    workspace: &Workspace,
    configuration: &Configuration,
) -> Result<(Model, Duration, Duration), Error> {
    let started = Instant::now();
    let bytes = indexing::index(workspace, configuration)?;
    let indexed = started.elapsed();
    let modelling = Instant::now();
    let index =
        scip::read(&bytes).map_err(|e| Error::new(format!("rust-analyzer's index: {e}")))?;
    let model = model::build(&index, workspace)?;
    Ok((model, indexed, modelling.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(words: &[&str]) -> Result<Options, String> {
        let arguments: Vec<String> = words.iter().map(|&word| word.to_owned()).collect();
        parse_arguments(&arguments)
    }

    fn configuration(words: &[&str]) -> Configuration {
        parsed(words).unwrap().configuration
    }

    #[test]
    fn no_flags_choose_the_default_configuration() {
        let options = parsed(&["my-crate"]).unwrap();
        assert_eq!(options.scope.as_deref(), Some("my-crate"));
        assert_eq!((options.allow, options.update_allow), (None, None));
        assert!(!options.ratchet && !options.timings && !options.all_ties);
        assert!(options.configuration.is_default());
    }

    #[test]
    fn a_value_follows_its_flag_or_an_equals_sign_in_it() {
        let spaced = [
            "my-crate",
            "--allow",
            "a.allow",
            "--target",
            "aarch64-apple-darwin",
        ];
        let joined = [
            "--allow=a.allow",
            "--target=aarch64-apple-darwin",
            "my-crate",
        ];
        for words in [&spaced[..], &joined[..]] {
            let options = parsed(words).unwrap();
            assert_eq!(options.scope.as_deref(), Some("my-crate"));
            assert_eq!(options.allow, Some(PathBuf::from("a.allow")));
            assert_eq!(
                options.configuration.to_string(),
                "--target aarch64-apple-darwin"
            );
        }
        for words in [
            &["--update-allow", "b.allow"][..],
            &["--update-allow=b.allow"],
        ] {
            let options = parsed(words).unwrap();
            assert_eq!(options.update_allow, Some(PathBuf::from("b.allow")));
        }
    }

    #[test]
    fn features_are_written_as_cargo_takes_them() {
        let expected = configuration(&["--features", "a,b"]);
        for words in [
            &["--features", "a b"][..],
            &["--features= a , b "],
            &["--features", "a", "--features", "b"],
            &["-F", "a", "-Fb"],
            &["-F=a,b"],
        ] {
            assert_eq!(configuration(words), expected, "{words:?}");
        }
        assert!(configuration(&["--features="]).is_default());
        assert_eq!(
            configuration(&["--no-default-features", "--all-features"]).to_string(),
            "--all-features --no-default-features"
        );
    }

    #[test]
    fn two_targets_are_an_error_since_rust_analyzer_indexes_one() {
        let two = [
            "--target",
            "aarch64-apple-darwin",
            "--target=x86_64-pc-windows-msvc",
        ];
        let error = parsed(&two).err().unwrap();
        assert!(error.contains("one target per run"), "{error}");
        let same = [
            "--target",
            "aarch64-apple-darwin",
            "--target=aarch64-apple-darwin",
        ];
        assert_eq!(
            configuration(&same).to_string(),
            "--target aarch64-apple-darwin"
        );
    }

    #[test]
    fn what_cargo_would_reject_is_an_error() {
        for words in [
            &["--target"][..],
            &["--target="],
            &["--features"],
            &["--target", "--features", "a"],
            &["--all-features", "--all-features"],
            &["--no-default-features=true"],
            &["--allow", "a", "--allow", "b"],
            &["--allow", "a", "--update-allow", "b"],
            &["--ratchet"],
            &["--allow", "a", "--ratchet", "--ratchet"],
            &["my-crate", "--update-allow", "a"],
            &["my-crate", "--allow", "a", "--ratchet"],
            &["--release"],
            &["-p", "my-crate"],
            &["my-crate", "other-crate"],
        ] {
            assert!(parsed(words).is_err(), "{words:?}");
        }
    }
}
