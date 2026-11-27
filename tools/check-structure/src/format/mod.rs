//! `rustc`/`clippy`-style diagnostic formatting, in color when stdout is a
//! terminal, with a picture under each report.

mod picture;

use std::fmt::Write;
use std::io::IsTerminal;

use crate::TOOL;
use crate::lint::Diagnostic;

pub fn render(
    diagnostics: &[Diagnostic],
    fixed_allowed: usize,
    allow_path: Option<&str>,
    all_ties: bool,
) -> String {
    let paint = Paint(stdout_has_color());
    let annotate = std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true");
    let mut out = String::new();
    for diag in diagnostics {
        if annotate {
            out.push_str(&annotation(diag));
        }
        let _ = writeln!(
            out,
            "{}{}",
            paint.error(&format!("error[{}]", diag.lint.name())),
            paint.bold(&format!(": {}", diag.message))
        );
        if let Some(loc) = &diag.location {
            // rustc shape (`file:line:column`) so IDE terminals link-navigate.
            let _ = writeln!(out, "  {} {}:{}:1", paint.gutter("-->"), loc.file, loc.line);
        }
        let more = diag.more_ties.len();
        let shown_more = if all_ties { &diag.more_ties[..] } else { &[] };
        for note in diag.notes.iter().chain(shown_more) {
            let _ = writeln!(
                out,
                "  {} {} {note}",
                paint.gutter("="),
                paint.bold("note:")
            );
        }
        if more > 0 && !all_ties {
            let noun = if more == 1 { "tie" } else { "ties" };
            let _ = writeln!(
                out,
                "  {} {} and {more} more {noun} (all of them with `--all-ties`)",
                paint.gutter("="),
                paint.bold("note:")
            );
        }
        for line in picture::draw(&diag.picture, &paint) {
            let _ = writeln!(out, "{line}");
        }
        out.push('\n');
    }

    if fixed_allowed > 0
        && let Some(path) = allow_path
    {
        let noun = if fixed_allowed == 1 {
            "entry"
        } else {
            "entries"
        };
        let _ = writeln!(
            out,
            "{} {fixed_allowed} allowed {noun} in {path} are now fixed or smaller (run `{TOOL} --update-allow {path}` to tighten it)",
            paint.bold("note:")
        );
    }

    if diagnostics.is_empty() {
        let _ = writeln!(out, "{TOOL}: {}", paint.ok("ok"));
    } else {
        let noun = if diagnostics.len() == 1 {
            "error"
        } else {
            "errors"
        };
        let count = format!("{} {noun}", diagnostics.len());
        let _ = writeln!(out, "{TOOL}: {}, well done!", paint.error(&count));
    }
    out
}

/// The workflow command that puts `diag` on the pull request's files: GitHub
/// reads `::error file=..,line=..,title=..::message` from the log.
fn annotation(diag: &Diagnostic) -> String {
    let escape = |text: &str| {
        text.replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    };
    // A property also ends at a comma and splits at a colon.
    let property = |text: &str| escape(text).replace(':', "%3A").replace(',', "%2C");
    let place = diag.location.as_ref().map_or_else(String::new, |loc| {
        format!("file={},line={},", property(&loc.file), loc.line)
    });
    format!(
        "::error {place}title={}::{}\n",
        property(diag.lint.name()),
        escape(&diag.message)
    )
}

/// Like cargo: `CARGO_TERM_COLOR=always|never` decides, then `NO_COLOR`, then
/// whether stdout is a terminal.
fn stdout_has_color() -> bool {
    match std::env::var("CARGO_TERM_COLOR").as_deref() {
        Ok("always") => true,
        Ok("never") => false,
        _ => {
            std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                && std::io::stdout().is_terminal()
        }
    }
}

/// rustc's colors: bold red errors, bold blue gutter, bold green success.
struct Paint(bool);

impl Paint {
    fn error(&self, text: &str) -> String {
        self.sgr("1;31", text)
    }

    fn gutter(&self, text: &str) -> String {
        self.sgr("1;34", text)
    }

    fn ok(&self, text: &str) -> String {
        self.sgr("1;32", text)
    }

    fn bold(&self, text: &str) -> String {
        self.sgr("1", text)
    }

    fn sgr(&self, code: &str, text: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::{Lint, Picture};

    fn circle_with_hidden_ties() -> Diagnostic {
        Diagnostic {
            lint: Lint::ModuleCycle,
            key: "p::a,p::b".to_owned(),
            message: "2 modules in `p` use each other in a circle".to_owned(),
            location: None,
            notes: vec!["first tie".to_owned()],
            more_ties: vec!["second tie".to_owned(), "third tie".to_owned()],
            picture: Picture::Circle {
                steps: Vec::new(),
                rest: None,
            },
        }
    }

    #[test]
    fn an_annotation_escapes_what_ends_a_property() {
        let mut diag = circle_with_hidden_ties();
        diag.location = Some(crate::lint::Location {
            file: "src/a,b:c.rs".to_owned(),
            line: 3,
        });
        let line = annotation(&diag);
        assert!(line.starts_with("::error file=src/a%2Cb%3Ac.rs,line=3,title=module-cycle::"));
    }

    #[test]
    fn hidden_ties_are_counted_unless_all_are_asked_for() {
        let diagnostics = [circle_with_hidden_ties()];
        let counted = render(&diagnostics, 0, None, false);
        assert!(counted.contains("note: and 2 more ties (all of them with `--all-ties`)"));
        assert!(!counted.contains("second tie"));
        let all = render(&diagnostics, 0, None, true);
        assert!(all.contains("note: second tie") && all.contains("note: third tie"));
        assert!(!all.contains("more ties"));
    }
}
