"""Exercise scripts/env-check.sh against stub toolchains.

Every case runs the script copied into a scratch repository, with a PATH that
holds only stubs and the few system tools the script itself needs, so the
verdicts never depend on the Rust, Node or Clang installed on the machine that
runs the suite.
"""

import importlib.util
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

SCRIPT = support.ROOT / "scripts/env-check.sh"
# The system tools the script may use; nothing that could pull the real
# toolchain in is linked.
SYSTEM_TOOLS = ("sed", "grep", "tr", "od", "dirname", "cat", "wc")
OPTIONAL = ("ruff", "shellcheck", "dash", "actionlint", "cargo-audit", "cargo-deny")
INSTALL_UNIX = "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
AGAIN = "./scripts/env-check.sh"


class Sandbox:
    """A scratch repository, a home directory and a PATH of stubs."""

    def __init__(self, tmp):
        self.tmp = Path(tmp)
        self.repo = self.tmp / "repo"
        self.home = self.tmp / "home"
        self.cargo_bin = self.home / ".cargo/bin"
        self.system = self.tmp / "system"
        for directory in (
            self.repo / "scripts",
            self.repo / "web/vendor",
            self.cargo_bin,
            self.system,
        ):
            directory.mkdir(parents=True)
        shutil.copy(SCRIPT, self.repo / "scripts/env-check.sh")
        self.pin("1.100.0")
        missing = support.link_system_tools(self.system, SYSTEM_TOOLS)
        if missing:
            raise unittest.SkipTest(f"{missing} not installed")
        self.stub("uname", "uname.sh", self.system)
        self.stub("cargo", "cargo.sh", self.cargo_bin)
        self.stub("rustc", "rustc.sh", self.cargo_bin)
        self.stub("node", "tool.sh", self.system)
        self.clang = self.tmp / "clang/bin/clang++"
        self.clang.parent.mkdir(parents=True)
        self.stub_at(self.clang, "clang.sh")

    def pin(self, channel):
        (self.repo / "rust-toolchain.toml").write_text(
            f'[toolchain]\nchannel = "{channel}"\ncomponents = ["clippy", "rustfmt"]\n'
        )

    def stub_at(self, path, source):
        support.write_executable(path, support.stub(f"env-check/{source}"))

    def stub(self, name, source, directory):
        self.stub_at(directory / name, source)

    def remove(self, name, directory=None):
        (directory or self.system).joinpath(name).unlink()

    def run(self, *args, path=None, env=None, remove_cxx=False):
        entries = path if path is not None else [self.cargo_bin, self.system]
        environment = {
            "PATH": os.pathsep.join(str(entry) for entry in entries),
            "HOME": str(self.home),
            "CXX": str(self.clang),
        }
        if remove_cxx:
            del environment["CXX"]
        environment.update(env or {})
        return subprocess.run(
            ["/bin/sh", str(self.repo / "scripts/env-check.sh"), *args],
            cwd=self.repo,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )


class EnvCheckCase(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.box = Sandbox(directory.name)

    def assertFailed(self, result, *needles):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for needle in needles:
            self.assertIn(needle, result.stdout)

    def lines(self, result, prefix):
        return [line for line in result.stdout.splitlines() if line.startswith(prefix)]


class LocalRust(EnvCheckCase):
    def test_a_complete_toolchain_passes(self):
        result = self.box.run()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.lines(result, "fail:"), [])

    def test_missing_cargo_prints_the_install_command(self):
        self.box.remove("cargo", self.box.cargo_bin)
        result = self.box.run()
        self.assertFailed(result, "fail: cargo", INSTALL_UNIX, AGAIN)

    def test_windows_points_at_rustup_init(self):
        self.box.remove("cargo", self.box.cargo_bin)
        result = self.box.run(env={"FAKE_UNAME_S": "MINGW64_NT-10.0"})
        self.assertFailed(result, "fail: cargo", "rustup-init.exe", AGAIN)
        self.assertNotIn("sh.rustup.rs", result.stdout)

    def test_a_version_other_than_the_pin_fails_with_both_numbers(self):
        result = self.box.run(env={"FAKE_RUSTC_VERSION": "1.99.0"})
        self.assertFailed(result, "fail: rustc", "1.99.0", "1.100.0")

    def test_a_failing_rustc_fails_for_a_named_pin_too(self):
        self.box.pin("nightly")
        result = self.box.run(env={"FAKE_RUSTC_BROKEN": "1"})
        self.assertFailed(result, "fail: rustc", "rustc -V failed")

    def test_a_named_pin_accepts_a_rustc_line_with_no_number(self):
        self.box.pin("nightly")
        result = self.box.run(env={"FAKE_RUSTC_ODD": "1"})
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("ok: rustc", result.stdout)

    def test_rustup_toolchain_overrides_the_pin(self):
        result = self.box.run(env={"RUSTUP_TOOLCHAIN": "nightly"})
        self.assertFailed(result, "fail: RUSTUP_TOOLCHAIN", "nightly")

    def test_a_matching_rustc_outside_cargo_home_only_warns(self):
        self.box.remove("rustc", self.box.cargo_bin)
        elsewhere = self.box.tmp / "distro"
        elsewhere.mkdir()
        self.box.stub("rustc", "rustc.sh", elsewhere)
        path = [self.box.cargo_bin, elsewhere, self.box.system]
        result = self.box.run(path=path)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(self.lines(result, "warning:")), 1)
        self.assertIn("rustc", self.lines(result, "warning:")[0])

    def test_a_mismatched_rustc_outside_cargo_home_still_fails(self):
        self.box.remove("rustc", self.box.cargo_bin)
        elsewhere = self.box.tmp / "distro"
        elsewhere.mkdir()
        self.box.stub("rustc", "rustc.sh", elsewhere)
        path = [self.box.cargo_bin, elsewhere, self.box.system]
        result = self.box.run(path=path, env={"FAKE_RUSTC_VERSION": "1.88.0"})
        self.assertFailed(result, "fail: rustc", "1.88.0")

    def test_cargo_home_names_where_the_shims_live(self):
        shims = self.box.tmp / "elsewhere/bin"
        shims.parent.mkdir()
        self.box.cargo_bin.rename(shims)
        result = self.box.run(
            path=[shims, self.box.system],
            env={"CARGO_HOME": str(shims.parent)},
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.lines(result, "warning:"), [])

    def test_a_missing_component_prints_the_rustup_command(self):
        for missing in ("clippy", "fmt"):
            with self.subTest(missing=missing):
                result = self.box.run(env={"FAKE_CARGO_MISSING": missing})
                self.assertFailed(result, "rustup component add clippy rustfmt")

    def test_a_cargo_that_cannot_report_its_version_fails(self):
        result = self.box.run(env={"FAKE_CARGO_BROKEN": "1"})
        self.assertFailed(result, "fail: cargo")

    def test_a_missing_pin_file_is_an_indeterminate_result(self):
        (self.box.repo / "rust-toolchain.toml").unlink()
        result = self.box.run()
        self.assertEqual(result.returncode, 2, result.stdout)
        self.assertIn("rust-toolchain.toml", result.stderr)

    def test_vendored_assets_must_be_present(self):
        shutil.rmtree(self.box.repo / "web/vendor")
        result = self.box.run()
        self.assertFailed(result, "fail: web/vendor")


class Rustup(EnvCheckCase):
    """With rustup on PATH nothing may run in the repository before the pinned
    toolchain is known to be installed, since rustup installs it on first use."""

    INSTALLED = (
        "stable-x86_64-unknown-linux-gnu (default)\n"
        "1.99.1-x86_64-unknown-linux-gnu\n"
        "1.100.0-x86_64-unknown-linux-gnu (active)"
    )
    INSTALL = "rustup toolchain install 1.100.0 --profile minimal -c clippy -c rustfmt"

    def setUp(self):
        super().setUp()
        self.box.stub("rustup", "rustup.sh", self.box.cargo_bin)
        self.marker = self.box.tmp / "ran"

    def run_with(self, listing, *args, env=None):
        environment = {
            "FAKE_RUSTUP_LIST": listing,
            "FAKE_FORBID_RUN": str(self.marker),
        }
        return self.box.run(*args, env=environment | (env or {}))

    def test_an_installed_pin_lets_the_checks_run(self):
        result = self.box.run(env={"FAKE_RUSTUP_LIST": self.INSTALLED})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ok: toolchain: 1.100.0 is installed", result.stdout)
        self.assertIn("ok: rustc: 1.100.0", result.stdout)

    def test_a_missing_pin_fails_without_running_cargo_or_rustc(self):
        for profile in ("local-rust", "portable"):
            with self.subTest(profile=profile):
                listing = "stable-x86_64-unknown-linux-gnu (default)\n1.99.1-x86_64"
                result = self.run_with(listing, "--profile", profile)
                self.assertFailed(result, "fail: toolchain", self.INSTALL, AGAIN)
                self.assertFalse(self.marker.exists())

    def test_a_missing_pin_is_reported_in_json_too(self):
        result = self.run_with("stable-x86_64-unknown-linux-gnu", "--format", "json")
        self.assertEqual(result.returncode, 1)
        report = json.loads(result.stdout)
        entry = next(e for e in report["required"] if e["name"] == "toolchain")
        self.assertIs(entry["ok"], False)
        self.assertFalse(self.marker.exists())

    def test_a_longer_name_does_not_stand_in_for_the_pin(self):
        result = self.run_with("1.100.01-x86_64-unknown-linux-gnu\n11.100.0-x86_64")
        self.assertFailed(result, "fail: toolchain")
        self.assertFalse(self.marker.exists())

    def test_a_named_channel_matches_by_its_full_name(self):
        self.box.pin("nightly-2026-01-01")
        listing = "nightly-2026-01-01-x86_64-unknown-linux-gnu"
        found = self.box.run("--profile", "portable", env={"FAKE_RUSTUP_LIST": listing})
        self.assertEqual(found.returncode, 0, found.stdout)
        self.assertFailed(
            self.run_with("nightly-x86_64-unknown-linux-gnu", "--profile", "portable"),
            "fail: toolchain",
        )

    def test_an_override_stops_the_shims_from_running(self):
        result = self.run_with(self.INSTALLED, env={"RUSTUP_TOOLCHAIN": "nightly"})
        self.assertFailed(result, "fail: RUSTUP_TOOLCHAIN")
        self.assertFalse(self.marker.exists())

    def test_a_failing_rustup_is_an_indeterminate_result(self):
        result = self.box.run(env={"FAKE_RUSTUP_BROKEN": "1"})
        self.assertEqual(result.returncode, 2, result.stdout)
        self.assertTrue(result.stderr.strip())

    def test_the_browser_profile_never_asks_rustup(self):
        result = self.run_with("", "--profile", "browser")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.marker.exists())


class Compiler(EnvCheckCase):
    def test_clang_21_is_accepted_and_20_is_not(self):
        self.assertEqual(self.box.run().returncode, 0)
        result = self.box.run(env={"FAKE_CLANG_VERSION": "20"})
        self.assertFailed(result, "fail: CXX", "20")

    def test_a_cxx_that_is_not_clang_is_refused(self):
        self.box.stub_at(self.box.clang, "tool.sh")
        self.assertFailed(self.box.run(), "fail: CXX")

    def test_a_linux_host_without_a_fetched_clang_must_name_a_compiler(self):
        machine = {"FAKE_UNAME_S": "Linux", "FAKE_UNAME_M": "aarch64"}
        result = self.box.run(env=machine, remove_cxx=True)
        self.assertFailed(result, "set CXX to a Clang 21+")

    def test_another_platform_builds_with_its_own_compiler(self):
        # webrtc-sys asks for Clang 21 only on Linux; macOS builds with the
        # system compiler, whatever version number it reports.
        machine = {"FAKE_UNAME_S": "Darwin", "FAKE_UNAME_M": "arm64"}
        result = self.box.run(env=machine, remove_cxx=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("CXX", result.stdout)

    def test_linux_without_a_compiler_prints_the_fetch_command(self):
        result = self.box.run(remove_cxx=True)
        self.assertFailed(result, "sh scripts/fetch-clang.sh target/clang")
        self.assertFalse((self.box.repo / "target").exists())

    def test_will_provision_turns_the_missing_compiler_into_a_note(self):
        result = self.box.run("--will-provision", remove_cxx=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(self.lines(result, "note:")), 1)
        self.assertFalse((self.box.repo / "target").exists())

    def test_will_provision_does_not_excuse_a_linux_host_that_cannot_fetch(self):
        result = self.box.run(
            "--will-provision",
            env={"FAKE_UNAME_S": "Linux", "FAKE_UNAME_M": "aarch64"},
            remove_cxx=True,
        )
        self.assertFailed(result, "set CXX to a Clang 21+")

    def test_a_checkout_path_with_a_space_still_finds_the_fetched_compiler(self):
        spaced = self.box.tmp / "Code Trial"
        shutil.copytree(self.box.repo, spaced)
        fetched = spaced / "target/clang/bin"
        fetched.mkdir(parents=True)
        self.box.stub_at(fetched / "clang++", "clang.sh")
        path = os.pathsep.join((str(self.box.cargo_bin), str(self.box.system)))
        result = subprocess.run(
            ["/bin/sh", str(spaced / "scripts/env-check.sh")],
            cwd=spaced,
            env={"PATH": path, "HOME": str(self.box.home)},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_the_fetched_compiler_is_used_when_cxx_is_unset(self):
        fetched = self.box.repo / "target/clang/bin"
        fetched.mkdir(parents=True)
        self.box.stub_at(fetched / "clang++", "clang.sh")
        result = self.box.run(remove_cxx=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.lines(result, "note:"), [])


class Optional(EnvCheckCase):
    def test_absent_tools_are_skipped_not_failed(self):
        result = self.box.run()
        self.assertEqual(result.returncode, 0)
        skipped = self.lines(result, "skipped:")
        for tool in OPTIONAL:
            with self.subTest(tool=tool):
                self.assertEqual(
                    len([line for line in skipped if f": {tool}: " in line]), 1
                )

    def test_present_tools_are_not_listed(self):
        for tool in OPTIONAL:
            self.box.stub(tool, "tool.sh", self.box.system)
        self.box.stub("docker", "tool.sh", self.box.system)
        result = self.box.run()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.lines(result, "skipped:"), [])

    def test_dash_alone_being_absent_is_reported_and_does_not_fail(self):
        for tool in OPTIONAL:
            if tool != "dash":
                self.box.stub(tool, "tool.sh", self.box.system)
        self.box.stub("docker", "tool.sh", self.box.system)
        result = self.box.run()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(
            self.lines(result, "skipped:"),
            [
                "skipped: dash: `sh -n` runs under the system sh, which may "
                "accept what dash refuses"
            ],
        )

    def test_the_skip_reasons_keep_the_wording_the_gate_printed(self):
        result = self.box.run()
        self.assertIn(
            "skipped: ruff: installing it lints the Python under scripts/, tests/ "
            "and the skills",
            result.stdout.splitlines(),
        )
        self.assertIn(
            "skipped: cargo-audit: `cargo install cargo-audit` audits the "
            "dependency tree",
            result.stdout.splitlines(),
        )


class Profiles(EnvCheckCase):
    def test_portable_needs_rustfmt_and_node_but_no_clippy_or_compiler(self):
        result = self.box.run(
            "--profile",
            "portable",
            env={"FAKE_CARGO_MISSING": "clippy", "FAKE_RUSTC_VERSION": "1.1.0"},
            remove_cxx=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = self.box.run(
            "--profile", "portable", env={"FAKE_CARGO_MISSING": "fmt"}
        )
        self.assertFailed(result, "rustup component add")

    def test_browser_needs_node_and_nothing_else(self):
        self.box.remove("cargo", self.box.cargo_bin)
        self.box.remove("rustc", self.box.cargo_bin)
        shutil.rmtree(self.box.repo / "web/vendor")
        result = self.box.run("--profile", "browser", remove_cxx=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.box.remove("node")
        self.assertFailed(self.box.run("--profile", "browser"), "fail: node")

    def test_ci_rust_needs_mold_and_its_linker_flag(self):
        flags = "-Clink-arg=-fuse-ld=mold"
        self.box.stub("mold", "tool.sh", self.box.system)
        ok = self.box.run("--profile", "ci-rust", env={"RUSTFLAGS": flags})
        self.assertEqual(ok.returncode, 0, ok.stdout + ok.stderr)
        self.assertFailed(self.box.run("--profile", "ci-rust"), "fail: RUSTFLAGS")
        self.box.remove("mold")
        self.assertFailed(
            self.box.run("--profile", "ci-rust", env={"RUSTFLAGS": flags}),
            "fail: mold",
        )

    def test_local_rust_does_not_ask_for_mold(self):
        self.assertEqual(self.box.run("--profile", "local-rust").returncode, 0)

    def test_unknown_profile_flag_and_format_are_invalid_invocations(self):
        for args in (
            ("--profile", "gpu"),
            ("--profile",),
            ("--format", "xml"),
            ("--nope",),
        ):
            with self.subTest(args=args):
                result = self.box.run(*args)
                self.assertEqual(result.returncode, 2, result.stdout)
                self.assertTrue(result.stderr.strip())


class Json(EnvCheckCase):
    def report(self, *args, env=None, **options):
        result = self.box.run("--format", "json", *args, env=env, **options)
        return result, json.loads(result.stdout)

    def test_stdout_is_one_object_and_nothing_else(self):
        result, report = self.report()
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(result.stdout.splitlines()), 1)
        self.assertEqual(set(report), {"profile", "ok", "required", "skipped"})
        self.assertEqual(report["profile"], "local-rust")
        self.assertIs(report["ok"], True)
        for entry in report["required"]:
            self.assertEqual(set(entry), {"name", "ok", "found", "want"})
        names = [entry["name"] for entry in report["required"]]
        self.assertIn("cargo", names)
        self.assertTrue(any(item.startswith("ruff: ") for item in report["skipped"]))

    def test_a_failure_is_reported_and_exits_one(self):
        self.box.remove("cargo", self.box.cargo_bin)
        result, report = self.report()
        self.assertEqual(result.returncode, 1)
        self.assertIs(report["ok"], False)
        failed = [entry for entry in report["required"] if not entry["ok"]]
        self.assertIn("cargo", [entry["name"] for entry in failed])

    def test_hostile_values_come_out_as_escaped_json(self):
        for hostile in (
            'x"\n{',
            "tab\there",
            "ctl\x01\x02\x1f\x7f end",
            "back\\slash éα",
            "carriage\rreturn",
            'slow \\ " path \x01 é',
        ):
            with self.subTest(hostile=hostile):
                result, report = self.report(env={"RUSTUP_TOOLCHAIN": hostile})
                self.assertEqual(result.returncode, 1)
                entry = next(
                    item
                    for item in report["required"]
                    if item["name"] == "RUSTUP_TOOLCHAIN"
                )
                self.assertEqual(entry["found"], hostile)
                self.assertIs(entry["ok"], False)
                # Every control character is escaped, DEL included, not only
                # the ones JSON insists on.
                raw = result.stdout.rstrip("\n")
                self.assertFalse([c for c in raw if ord(c) < 32 or ord(c) == 127])

    def test_a_hostile_rustc_version_is_escaped_too(self):
        result, report = self.report(env={"FAKE_RUSTC_VERSION": 'v"\x01'})
        self.assertEqual(result.returncode, 1)
        found = [e["found"] for e in report["required"] if e["name"] == "rustc"]
        self.assertTrue(found and 'v"\x01' in found[0])


if __name__ == "__main__":
    unittest.main()
