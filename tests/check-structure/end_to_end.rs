//! End-to-end tests of `check-structure` on small workspaces through
//! `rust-analyzer scip`.
//! A fixture family is a base workspace in `fixtures/<family>` beside this file
//! and its variants in `variants/<family>/<variant>`, each holding only the
//! files it changes: restructurings that must report the same keys, and a fix.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TOOL: &str = "check-structure";
const PROGRAM: &str = env!("CARGO_BIN_EXE_check-structure");

/// A copy of `fixture`, with `variant`'s files over it, to run `test` in.
fn workspace(fixture: &str, variant: Option<&str>, test: &str) -> PathBuf {
    let rust_analyzer = Command::new("rust-analyzer").arg("--version").output();
    assert!(
        rust_analyzer.is_ok_and(|output| output.status.success()),
        "these tests run rust-analyzer, which is not on PATH (the dev shell has it)"
    );
    let copy = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    if copy.exists() {
        std::fs::remove_dir_all(&copy).unwrap();
    }
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/check-structure");
    copy_directory(&tests.join("fixtures").join(fixture), &copy);
    if let Some(variant) = variant {
        copy_directory(&tests.join("variants").join(fixture).join(variant), &copy);
    }
    copy
}

fn copy_directory(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_directory(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(workspace: &Path, arguments: &[&str]) -> Run {
    run_with(workspace, &[], arguments)
}

/// A run with `variables` set in its environment, and the test's own.
fn run_with(workspace: &Path, variables: &[(&str, &str)], arguments: &[&str]) -> Run {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(PROGRAM)
        .args(arguments)
        .current_dir(workspace)
        // CI sets CARGO_TERM_COLOR=always; the tests read plain text.
        .env("CARGO_TERM_COLOR", "never")
        .envs(variables.iter().copied())
        .output()
        .unwrap();
    let run = Run {
        code: status.code(),
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    };
    let variables: Vec<String> = variables
        .iter()
        .map(|(name, value)| format!("{name}='{value}' "))
        .collect();
    eprintln!(
        "$ {}{TOOL} {}\n{}{}",
        variables.concat(),
        arguments.join(" "),
        run.stdout,
        run.stderr
    );
    run
}

/// `lint key` for every diagnostic in `workspace`, as the allow file lists
/// them.
fn keys(workspace: &Path) -> Vec<String> {
    configured_keys(workspace, &[])
}

/// The keys of the configuration that `flags` choose.
fn configured_keys(workspace: &Path, flags: &[&str]) -> Vec<String> {
    let allow = workspace.with_extension("keys");
    let mut arguments = vec!["--update-allow", allow.to_str().unwrap()];
    arguments.extend(flags);
    let update = run(workspace, &arguments);
    assert_eq!(update.code, Some(0));
    let mut keys: Vec<String> = std::fs::read_to_string(&allow)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    keys.sort();
    keys
}

/// The keys of `family`'s base, or of one of its variants.
fn variant_keys(family: &str, variant: Option<&str>) -> Vec<String> {
    let test = format!("{family}-{}", variant.unwrap_or("base"));
    keys(&workspace(family, variant, &test))
}

/// The contents of every file named `name` in `workspace`'s build directory,
/// where its build scripts' `OUT_DIR`s are.
fn built_files(workspace: &Path, name: &str) -> Vec<String> {
    let metadata = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace)
        .output()
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let mut files = Vec::new();
    let build_directory = metadata["build_directory"].as_str().unwrap();
    find_files(Path::new(build_directory), name, &mut files);
    files
}

fn find_files(directory: &Path, name: &str, files: &mut Vec<String>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            find_files(&path, name, files);
        } else if path.file_name().is_some_and(|file| file == name) {
            files.push(std::fs::read_to_string(&path).unwrap());
        }
    }
}

#[test]
fn clean_workspace_exits_zero() {
    let workspace = workspace("clean", None, "clean");
    let result = run(&workspace, &[]);
    assert_eq!(result.code, Some(0));
    assert!(result.stdout.contains(&format!("{TOOL}: ok")));
}

#[test]
fn a_leaked_field_leaks_however_it_is_reached_and_not_once_the_type_does_the_work() {
    let run_queue = run(&workspace("field_leak", None, "field_leak"), &[]);
    assert_eq!(run_queue.code, Some(1));
    assert!(run_queue.stdout.contains(
        "error[field-leak]: `RunQueue::items` is known outside `RunQueue`: 4 places in 2 modules"
    ));
    // `backlog` and `drained` only read `head`.
    assert!(run_queue.stdout.contains(
        "    runqueue::queue::ring::RunQueue\n\
         \x20     items ◀━┳━ 2  runqueue::sched::policy\n\
         \x20             ┗━ 2  runqueue::sched::report\n\
         \x20     also known outside: head\n\
         \x20     kept: limit\n"
    ));
    let leaked = [
        "field-leak runqueue::queue::ring::RunQueue::head",
        "field-leak runqueue::queue::ring::RunQueue::items",
    ];
    for variant in [
        "getters",
        "let_getters",
        "all_pub",
        "pasted",
        "tuple_getter",
    ] {
        assert_eq!(
            variant_keys("field_leak", Some(variant)),
            leaked,
            "{variant}"
        );
    }

    // The work moved into the type, or into a type behind a field that it
    // passes each call on to; and reading a copy of a value, with or without a
    // getter, works on nothing, even one the getter lends: `*queue.limit()`.
    for variant in ["real", "passed_on", "read", "lent_read"] {
        assert_eq!(
            variant_keys("field_leak", Some(variant)),
            Vec::<String>::new(),
            "{variant}"
        );
    }
}

#[test]
fn a_module_cycle_stays_one_through_a_re_export_or_a_relay_until_code_moves_where_it_is_used() {
    let net = run(&workspace("module_cycle", None, "module_cycle"), &[]);
    assert_eq!(net.code, Some(1));
    assert!(
        net.stdout
            .contains("error[module-cycle]: 2 modules in `net` use each other in a circle")
    );
    assert!(!net.stdout.contains("error[tangle]"));
    // Things in a chain: moving `BACKLOG_MOST` to its one user breaks it.
    assert!(net.stdout.contains(
        "    ┌─▶ kinds\n\
         \x20   │     ┃ kinds.rs:10  Stream::new uses BACKLOG_MOST\n\
         \x20   │     ▼\n\
         \x20   │   socket\n\
         \x20   │     │ socket.rs:5  listen uses Stream::{self, new}\n\
         \x20   └─────┘\n"
    ));
    assert_eq!(
        variant_keys("module_cycle", Some("reexported")),
        ["module-cycle net::kinds,net::socket"]
    );
    assert_eq!(
        variant_keys("module_cycle", Some("relayed")),
        ["module-cycle net::kinds,net::limits,net::socket"]
    );
    assert_eq!(
        variant_keys("module_cycle", Some("moved")),
        Vec::<String>::new()
    );
}

#[test]
fn a_module_built_only_for_tests_is_test_code_whatever_its_name() {
    // `kinds` declares `#[cfg(test)] mod harness;`, whose `fixtures` call
    // `socket`, and `#[cfg(all(test, not(miri)))] mod probe { .. }`, which does
    // too. Test code makes no circle with `socket`.
    let workspace = workspace("module_cycle", Some("for_tests"), "module_cycle-for_tests");
    assert_eq!(keys(&workspace), Vec::<String>::new());

    // Built for more than the tests, either one is code of `kinds` that uses
    // `socket`.
    let kinds = workspace.join("src/kinds.rs");
    let text = std::fs::read_to_string(&kinds).unwrap();
    for (attribute, instead) in [
        ("#[cfg(test)]", "#[cfg(any(test, miri))]"),
        ("#[cfg(all(test, not(miri)))]", ""),
    ] {
        std::fs::write(&kinds, text.replace(attribute, instead)).unwrap();
        assert_eq!(
            keys(&workspace),
            ["module-cycle net::kinds,net::socket"],
            "{attribute} as {instead:?}"
        );
    }

    // An example builds a module at the same path for itself: the module is not
    // test code, the library's code in it included.
    std::fs::write(&kinds, &text).unwrap();
    let demo = workspace.join("examples/demo");
    std::fs::create_dir_all(demo.join("kinds")).unwrap();
    std::fs::write(
        demo.join("main.rs"),
        "mod kinds;\nfn main() {\n    kinds::harness::run();\n}\n",
    )
    .unwrap();
    std::fs::write(demo.join("kinds.rs"), "pub mod harness;\n").unwrap();
    std::fs::write(demo.join("kinds/harness.rs"), "pub fn run() {}\n").unwrap();
    assert_eq!(keys(&workspace), ["module-cycle net::kinds,net::socket"]);
}

#[test]
fn indexing_runs_the_build_scripts_so_a_cycle_through_what_they_write_is_seen() {
    let workspace = workspace("build_script", None, "build_script");
    let net = run(&workspace, &[]);
    // The first index of a fresh copy ran the build script...
    assert_eq!(
        built_files(&workspace, "limits.rs"),
        ["pub const BACKLOG_MOST: usize = 16;\n"]
    );

    // ...and `socket` includes `BACKLOG_MOST`, which no source file defines,
    // from what it wrote.
    assert_eq!(net.code, Some(1));
    assert!(net.stdout.contains(
        "    ┌─▶ kinds\n\
         \x20   │     ┃ kinds.rs:10  Stream::new uses BACKLOG_MOST\n\
         \x20   │     ▼\n\
         \x20   │   socket\n"
    ));
}

#[test]
fn a_tangle_stays_one_however_it_is_laid_out_until_it_is_one_module_or_untied() {
    let tasks_and_timers = "tangle boot::kernel::tasks::Tasks,boot::kernel::timers::Timers";
    let cycle = "module-cycle boot::kernel::tasks,boot::kernel::timers";
    let boot = run(&workspace("tangle", None, "tangle"), &[]);
    assert!(
        boot.stdout
            .contains("error[tangle]: 2 things know each other in a circle, across 2 modules")
    );
    assert!(boot.stdout.contains(
        "    ┌─▶ boot::kernel::tasks::Tasks\n\
         \x20   │     ┃ tasks.rs:17   Tasks::spawn uses Timers::{self, arm}\n\
         \x20   │     ▼\n\
         \x20   │   boot::kernel::timers::Timers\n\
         \x20   │     │ timers.rs:16  Timers::fire uses Tasks::{self, wake}\n\
         \x20   └─────┘\n"
    ));
    assert!(
        boot.stdout.contains(
            "error[module-cycle]: 2 modules in `boot::kernel` use each other in a circle"
        )
    );

    // Moving an `impl` block or lifting a type into the parent's file puts the
    // modules in order, and leaves the tangle.
    assert_eq!(
        variant_keys("tangle", Some("impl_moved")),
        [tasks_and_timers]
    );
    assert_eq!(
        variant_keys("tangle", Some("lifted")),
        ["tangle boot::kernel::Tasks,boot::kernel::timers::Timers"]
    );

    // The figure's fix: `Tasks` stays lifted, and `tick` unties it from
    // `Timers`.
    for variant in ["merged", "real", "lifted_untied"] {
        assert_eq!(
            variant_keys("tangle", Some(variant)),
            Vec::<String>::new(),
            "{variant}"
        );
    }

    let ring = workspace("tangle", Some("ring"), "tangle-ring");
    let result = run(&ring, &[]);
    assert!(
        result
            .stdout
            .contains("error[tangle]: 8 things know each other in a circle, all in `boot::ring`")
    );
    let ring_key = "tangle boot::ring::A,boot::ring::B,boot::ring::C,boot::ring::D,\
                    boot::ring::E,boot::ring::F,boot::ring::G,boot::ring::H";
    assert_eq!(keys(&ring), [cycle, tasks_and_timers, ring_key]);
}

#[test]
fn a_shallow_module_stays_shallow_however_it_is_dressed_until_it_does_more_with_less() {
    let store = run(&workspace("shallow", None, "shallow"), &[]);
    assert!(store.stdout.contains(
        "error[shallow-module]: `inventory::store` is shallow: it shows 9 of its 15 functions"
    ));
    assert!(store.stdout.contains(
        "= note: `Store` shows 9 functions: Store::contains, Store::get, Store::insert, \
         Store::is_full, Store::largest, Store::new, Store::remove, +2 more"
    ));
    assert!(store.stdout.contains(
        "                  7\n\
         \x20   shows  ███████│██ 9\n\
         \x20   hides  ██████ │ 6\n\
         \x20   Store  ███████│██ 9\n"
    ));
    let shallow = ["shallow-module inventory::store"];

    // Some or all of its methods made free functions that use the store: they
    // are still the store's functions.
    for variant in [
        "padded",
        "impls_moved",
        "pub_field",
        "private_modules",
        "some_free_functions",
        "free_functions",
    ] {
        assert_eq!(variant_keys("shallow", Some(variant)), shallow, "{variant}");
    }
    // `is_full` names the store only in its signature.
    let free = run(
        &workspace("shallow", Some("free_functions"), "shallow-free"),
        &[],
    );
    assert!(free.stdout.contains(
        "= note: `Store` shows 9 functions: Store::new, contains, get, insert, is_full, \
         largest, remove, +2 more"
    ));
    assert_eq!(variant_keys("shallow", Some("deep")), Vec::<String>::new());
}

#[test]
fn small_types_that_each_show_a_few_functions_are_not_shallow_however_many_they_show_in_all() {
    let stats = run(&workspace("small_types", None, "small_types"), &[]);
    assert!(stats.stdout.contains(&format!("{TOOL}: ok")));
    assert_eq!(stats.code, Some(0));
}

#[test]
fn a_wide_directory_is_reported_until_it_is_split() {
    let workspace = workspace("clean", None, "layout");
    let socket = workspace.join("src/socket");
    let mod_rs = socket.join("mod.rs");
    let socket_mod = std::fs::read_to_string(&mod_rs).unwrap();
    let parts = |count: usize| {
        let declared: String = (1..=count).map(|n| format!("mod part{n};\n")).collect();
        std::fs::write(&mod_rs, declared + &socket_mod).unwrap();
        for n in 1..=count {
            let part = format!("pub struct Part{n};\n");
            std::fs::write(socket.join(format!("part{n}.rs")), part).unwrap();
        }
    };
    // 15 files: at the most, not past it.
    parts(13);
    assert_eq!(keys(&workspace), Vec::<String>::new());

    parts(14);
    let result = run(&workspace, &[]);
    assert_eq!(result.code, Some(1));
    assert!(result.stdout.contains(
        "error[wide-directory]: `src/socket/` is wide: it holds 16 files and directories, \
         more than 15\n\
         \x20 --> src/socket/mod.rs:1:1\n\
         \x20 = note: 16 files: datagram.rs, mod.rs, part1.rs, part10.rs, part11.rs, \
         part12.rs, part13.rs, +9 more\n\
         \x20                        15\n\
         \x20   holds  ███████████████│█ 16\n"
    ));
    assert_eq!(keys(&workspace), ["wide-directory src/socket/"]);
}

#[test]
fn the_allow_file_suppresses_only_existing_entries_and_notes_what_is_fixed() {
    let workspace = workspace("field_leak", None, "allow");
    let allow = workspace.with_extension("allow");
    let allow = allow.to_str().unwrap();
    assert_eq!(run(&workspace, &["--update-allow", allow]).code, Some(0));
    let allowed = run(&workspace, &["--allow", allow]);
    assert_eq!(allowed.code, Some(0));
    assert!(allowed.stdout.contains(&format!("{TOOL}: ok")));

    let real = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/check-structure/variants/field_leak/real");
    copy_directory(&real, &workspace);
    let fixed = run(&workspace, &["--allow", allow]);
    assert_eq!(fixed.code, Some(0));
    assert!(fixed.stdout.contains("2 allowed entries"));
    assert!(fixed.stdout.contains("now fixed"));

    let lib = workspace.join("src/lib.rs");
    let ring = "\npub mod ring {\n    pub struct A(pub Option<Box<B>>);\n    pub struct B(pub Option<Box<C>>);\n\
                pub struct C(pub Option<Box<D>>);\n    pub struct D(pub Option<Box<E>>);\n\
                pub struct E(pub Option<Box<F>>);\n    pub struct F(pub Option<Box<G>>);\n\
                pub struct G(pub Option<Box<H>>);\n    pub struct H(pub Option<Box<A>>);\n}\n";
    let with_ring = std::fs::read_to_string(&lib).unwrap() + ring;
    std::fs::write(&lib, with_ring).unwrap();
    let new = run(&workspace, &["--allow", allow]);
    assert_eq!(new.code, Some(1));
    assert!(new.stdout.contains("error[tangle]: 8 things"));
    assert!(!new.stdout.contains("field-leak"));
}

#[test]
fn across_crates_a_field_leaks_and_test_code_is_left_out() {
    let workspace = workspace("two_crates", None, "two_crates");
    assert_eq!(run(&workspace, &[]).code, Some(0));

    let lib = workspace.join("crates/lib/src/lib.rs");
    let vault = "\npub struct Plain { pub x: u32, pub y: u32 }\n\
                 pub struct Vault { secret: u32, pub leaked: u32 }\n\
                 impl Vault {\n    pub fn new() -> Self { Self { secret: 1, leaked: 2 } }\n    \
                 pub fn next(&self) -> u32 { self.secret + 1 }\n}\n\
                 #[cfg(test)]\nmod tests {\n    #[test]\n    fn peeks() { let _ = super::Vault::new().leaked; }\n}\n";
    std::fs::write(&lib, std::fs::read_to_string(&lib).unwrap() + vault).unwrap();
    let it = workspace.join("app/tests/it.rs");
    let peeks = "\n#[test]\nfn peeks() {\n    let _ = lib::Vault::new().leaked;\n}\n";
    std::fs::write(&it, std::fs::read_to_string(&it).unwrap() + peeks).unwrap();

    let main = workspace.join("app/src/main.rs");
    let original = std::fs::read_to_string(&main).unwrap();
    let plain = format!("{original}\nfn bump_plain(p: &mut lib::Plain) {{ p.x += 1; }}\n");
    std::fs::write(&main, plain).unwrap();
    assert_eq!(keys(&workspace), Vec::<String>::new());
    let read = format!("{original}\nfn read_vault(v: &lib::Vault) -> u32 {{ v.leaked }}\n");
    std::fs::write(&main, read).unwrap();
    assert_eq!(keys(&workspace), Vec::<String>::new());

    let leak = format!("{original}\nfn bump_vault(v: &mut lib::Vault) {{ v.leaked += 1; }}\n");
    std::fs::write(&main, leak).unwrap();
    let result = run(&workspace, &[]);
    assert_eq!(result.code, Some(1));
    assert!(result.stdout.contains(
        "error[field-leak]: `Vault::leaked` is known outside `Vault`: 1 place in 1 module"
    ));
    assert_eq!(keys(&workspace), ["field-leak lib::Vault::leaked"]);
}

#[test]
fn integration_test_submodules_named_after_a_library_module_do_not_make_it_shallow() {
    let workspace = workspace("clean", None, "test_target_same_name");
    let lib = workspace.join("src/lib.rs");
    std::fs::write(
        &lib,
        "pub mod queue;\npub fn run() -> u32 {\n    \
         queue::f1() + queue::f2() + queue::f3() + queue::f4() \
         + queue::f5() + queue::f6() + queue::f7() + queue::f8()\n}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("src/queue.rs"),
        "pub fn f1() -> u32 { 1 }\npub fn f2() -> u32 { 2 }\n\
         pub fn f3() -> u32 { 3 }\npub fn f4() -> u32 { 4 }\n\
         pub fn f5() -> u32 { 5 }\npub fn f6() -> u32 { 6 }\n\
         pub fn f7() -> u32 { 7 }\npub fn f8() -> u32 { 8 }\n",
    )
    .unwrap();
    // Leaf module `src/queue.rs` is not judged by `shallow-module`.
    assert_eq!(keys(&workspace), Vec::<String>::new());

    // An integration test target declaring `mod queue;` with child module
    // `part` and an inline `mod helper` must not turn `src/queue.rs` into a
    // parent module judged by `shallow-module`, while layout checks still see
    // test files.
    std::fs::create_dir_all(workspace.join("tests/queue")).unwrap();
    std::fs::write(workspace.join("tests/main.rs"), "mod queue;\n").unwrap();
    std::fs::write(
        workspace.join("tests/queue/mod.rs"),
        "mod part;\nmod helper {\n    pub fn expected() -> u32 { 36 }\n}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("tests/queue/part.rs"),
        "#[test]\nfn runs() { assert_eq!(clean::run(), super::helper::expected()); }\n",
    )
    .unwrap();
    assert_eq!(keys(&workspace), Vec::<String>::new());

    // Layout checks still apply to directories inside `tests/queue/`.
    let declared: String = (0..16).map(|n| format!("mod more{n};\n")).collect();
    let mod_rs = workspace.join("tests/queue/mod.rs");
    let existing = std::fs::read_to_string(&mod_rs).unwrap();
    std::fs::write(&mod_rs, declared + &existing).unwrap();
    for n in 0..16 {
        std::fs::write(workspace.join(format!("tests/queue/more{n}.rs")), "").unwrap();
    }
    assert_eq!(keys(&workspace), ["wide-directory tests/queue/"]);
}

#[test]
fn a_binary_that_includes_another_binarys_root_changes_nothing_however_the_two_sort() {
    // `git-wt` sorts before `wt`, and `include!`s its root, `src/main.rs`.
    let wt = run(&workspace("included_root", None, "included_root"), &[]);
    assert_eq!(wt.code, Some(1));
    assert!(wt.stdout.contains(
        "error[field-leak]: `Args::verbose` is known outside `Args`: 1 place in 1 module\n\
         \x20 --> src/main.rs:5:1\n"
    ));
    let leaked = ["field-leak wt::cli::Args::verbose"];
    assert_eq!(variant_keys("included_root", None), leaked);
    assert_eq!(variant_keys("included_root", Some("renamed")), leaked);
}

#[test]
fn a_target_or_features_bring_in_the_code_behind_their_cfg_and_each_keeps_its_index() {
    let workspace = workspace("gated", None, "gated");
    let console = "module-cycle gated::console::handle,gated::console::modes";
    let export = "module-cycle gated::export::csv,gated::export::rows";
    let metrics = "module-cycle gated::metrics::counter,gated::metrics::sink";
    assert_eq!(keys(&workspace), [metrics]);

    // The Windows standard library need not be installed: rust-analyzer reads
    // its source.
    let windows = ["--target", "x86_64-pc-windows-msvc"];
    assert_eq!(configured_keys(&workspace, &windows), [console, metrics]);
    assert_eq!(
        configured_keys(&workspace, &["--features", "export"]),
        [export, metrics]
    );
    assert_eq!(
        configured_keys(&workspace, &["--all-features"]),
        [export, metrics]
    );
    assert_eq!(
        configured_keys(&workspace, &["--no-default-features"]),
        Vec::<String>::new()
    );

    let fresh = run(&workspace, &["-F", "export", "--no-default-features"]);
    assert_eq!(fresh.code, Some(1));
    assert!(fresh.stderr.contains(&format!(
        "{TOOL}: indexing workspace with --features export --no-default-features..."
    )));
    assert!(fresh.stdout.contains("2 modules in `gated::export`"));
    assert!(!fresh.stdout.contains("gated::metrics"));
    // Back to each: its index is still there, however its flags are written.
    for (flags, code) in [
        (&[][..], 1),
        (&windows, 1),
        (&["--features=export"], 1),
        (&["--all-features"], 1),
        (&["--no-default-features"], 0),
    ] {
        let again = run(&workspace, flags);
        assert!(!again.stderr.contains("indexing"), "{flags:?}");
        assert_eq!(again.code, Some(code), "{flags:?}");
    }
}

#[test]
fn cargo_s_own_configuration_chooses_the_target_and_cfgs_too_and_each_keeps_its_index() {
    // A directory of its own around the workspace, for the `.cargo/config.toml`
    // that cargo reads there, as it reads each ancestor's.
    let workspace = workspace("gated", None, "gated-cargo-config/gated");
    let config = workspace.parent().unwrap().join(".cargo");
    if config.exists() {
        std::fs::remove_dir_all(&config).unwrap();
    }
    std::fs::create_dir_all(&config).unwrap();
    let build_target = |triple: &str| {
        let toml = format!("[build]\ntarget = \"{triple}\"\n");
        std::fs::write(config.join("config.toml"), toml).unwrap();
    };
    let console = "2 modules in `gated::console`";
    let windows = ["--target", "x86_64-pc-windows-msvc"];
    let plain = run(&workspace, &[]);
    assert!(plain.stderr.contains("indexing workspace..."));
    assert!(!plain.stdout.contains(console));
    assert!(run(&workspace, &windows).stdout.contains(console));

    // Each brings in `console`, indexing once: the next run finds its index.
    let indexes_once = |variables: &[(&str, &str)]| {
        for indexes in [true, false] {
            let report = run_with(&workspace, variables, &[]);
            assert_eq!(
                report.stderr.contains("indexing workspace..."),
                indexes,
                "{variables:?}"
            );
            assert!(report.stdout.contains(console), "{variables:?}");
        }
    };
    build_target("x86_64-pc-windows-msvc");
    indexes_once(&[]);
    std::fs::remove_file(config.join("config.toml")).unwrap();
    indexes_once(&[("RUSTFLAGS", "--cfg windows")]);

    // `--target` overrides `build.target`, as on cargo's command line.
    build_target("aarch64-apple-darwin");
    let flagged = run(&workspace, &windows);
    assert!(!flagged.stderr.contains("indexing"));
    assert!(flagged.stdout.contains(console));
    // Without either, the first index is still there.
    std::fs::remove_file(config.join("config.toml")).unwrap();
    let again = run(&workspace, &[]);
    assert!(!again.stderr.contains("indexing"));
    assert_eq!(again.stdout, plain.stdout);
}

#[test]
fn a_target_or_a_feature_cargo_does_not_know_is_an_error_not_a_report() {
    let workspace = workspace("gated", None, "gated-unknown");
    let feature = run(&workspace, &["--features", "exports"]);
    assert_eq!(feature.code, Some(2));
    assert!(feature.stdout.is_empty());
    assert!(
        feature
            .stderr
            .contains("cannot index with `--features exports`: cargo metadata says: error:")
    );
    assert!(
        feature
            .stderr
            .contains("does not contain this feature: exports")
    );
    let target = run(&workspace, &["--target", "x86_64-pc-windows-mscv"]);
    assert_eq!(target.code, Some(2));
    assert!(target.stdout.is_empty());
    assert!(target.stderr.contains(
        "cannot index with `--target x86_64-pc-windows-mscv`: cargo metadata says: error:"
    ));
    assert!(!target.stderr.contains("indexing"));
}

#[test]
fn outside_a_cargo_workspace_reports_error() {
    let empty = std::env::temp_dir().join(format!("{TOOL}-not-a-workspace-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let run = run(&empty, &[]);
    std::fs::remove_dir_all(&empty).unwrap();
    assert_eq!(run.code, Some(2));
    assert!(run.stderr.contains("not inside a Cargo workspace"));
}

#[test]
fn a_ratchet_fails_on_a_fixed_entry_that_a_plain_allow_only_notes() {
    let workspace = workspace("clean", None, "ratchet");
    let allow = workspace.with_extension("allow");
    std::fs::write(&allow, "tangle ghost::module\n").unwrap();
    let allow = allow.to_str().unwrap();
    let plain = run(&workspace, &["--allow", allow]);
    assert_eq!(plain.code, Some(0));
    assert!(plain.stdout.contains("are now fixed"));

    let ratchet = run(&workspace, &["--allow", allow, "--ratchet"]);
    assert_eq!(ratchet.code, Some(1));
    assert!(
        ratchet.stderr.contains("1 allowed entries"),
        "{}",
        ratchet.stderr
    );
}

#[test]
fn github_actions_gets_an_annotation_for_each_error() {
    let workspace = workspace("field_leak", None, "annotation");
    let plain = run(&workspace, &[]);
    assert!(!plain.stdout.contains("::error"));
    let annotated = run_with(&workspace, &[("GITHUB_ACTIONS", "true")], &[]);
    assert!(
        annotated
            .stdout
            .contains("::error file=src/sched/policy.rs,line=7,title=field-leak::"),
        "{}",
        annotated.stdout
    );
}

#[test]
fn a_cycle_that_shrank_is_still_allowed_but_its_entry_is_stale_under_a_ratchet() {
    let workspace = workspace("module_cycle", None, "shrunk_cycle");
    let [found] = &keys(&workspace)[..] else {
        panic!("the fixture has one cycle");
    };

    // The entry an earlier, larger circle left behind: its members include the
    // ones the circle has now.
    let allow = workspace.with_extension("shrunk.allow");
    std::fs::write(&allow, format!("{found},ghost::module\n")).unwrap();
    let allow = allow.to_str().unwrap();

    let plain = run(&workspace, &["--allow", allow]);
    assert_eq!(plain.code, Some(0), "{}", plain.stdout);
    assert!(plain.stdout.contains("are now fixed or smaller"));

    let ratchet = run(&workspace, &["--allow", allow, "--ratchet"]);
    assert_eq!(ratchet.code, Some(1));
    assert!(
        ratchet.stderr.contains("fixed or smaller"),
        "{}",
        ratchet.stderr
    );
}
