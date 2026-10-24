"""Exercise CI lane selection without executing the expensive gate commands."""

import importlib.util
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

ROOT = support.ROOT
SCRIPT = ROOT / "scripts/test.sh"
# The events that start only the avatar job.
NOT_AVATAR = (
    "github.event_name != 'schedule' && github.event_name != 'workflow_dispatch'"
)


def gates_job():
    return support.job("gates")


def lanes():
    """Each lane's matrix entry, as a dict of its keys."""
    tail = support.section(gates_job(), "        include:\n", "\0", "matrix include")
    include = []
    for line in tail.splitlines():
        if not line.startswith("          "):
            break
        include.append(line)
    include = "\n".join(include)
    entries = {}
    for entry in include.split("          - ")[1:]:
        values = dict(re.findall(r"(\w+): (.+)", entry))
        entries[values["lane"]] = values
    return entries


def function_body(name):
    source = SCRIPT.read_text()
    return support.section(source, f"\n{name}()\n{{\n", "\n}\n", f"function {name}")


def condition(text, name):
    match = re.search(r"^        if: (.+)$", support.step(text, name), re.M)
    return match[1] if match else ""


def action_script(name):
    """The run script of a composite action under .github/actions."""
    return support.run_block((ROOT / f".github/actions/{name}/action.yml").read_text())


def stubbed(tmp, script, stubs):
    """A runner for script under bash -eo pipefail, the shell a composite
    `shell: bash` step gets, with stubs first on PATH.

    Each call takes the environment it adds and returns the lines the stubs
    logged to $CALLS. `timeout` is always stubbed to run its command.
    """
    bin_dir = Path(tmp) / "bin"
    bin_dir.mkdir()
    for name, body in {"timeout": 'shift; exec "$@"', **stubs}.items():
        support.write_executable(bin_dir / name, f"#!/bin/sh\n{body}\n")
    calls = Path(tmp) / "calls"

    def run(**env):
        calls.write_text("")
        subprocess.run(
            ["bash", "-eo", "pipefail", "-c", script],
            env=os.environ
            | {
                "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
                "CALLS": str(calls),
            }
            | env,
            check=True,
            capture_output=True,
        )
        return calls.read_text().splitlines()

    return run


class TestGateLanes(unittest.TestCase):
    def run_lane(
        self,
        *args,
        fail="",
        extra_gate=False,
        skip="",
        ci=False,
        unset_cxx=False,
        replace=None,
    ):
        source = SCRIPT.read_text()
        if replace:
            source = source.replace(*replace)
        # Keep the real dispatch and failure aggregation; substitute only the
        # gate payload so the test needs neither compilers nor downloaded assets.
        self.assertEqual(source.count('if "$@"; then'), 1)
        source = source.replace('if "$@"; then', 'if probe "$name"; then')
        source = source.replace(
            "gate()\n",
            """probe()
{
    echo "gate:$1"
    case " $PROBE_SKIP " in
        *" $1 "*) skip "$1" ;;
    esac
    case " $PROBE_FAIL " in
        *" $1 "*) return 1 ;;
    esac
}

gate()
""",
            1,
        )
        if extra_gate:
            source = source.replace("gate fmt ", "gate future-check true\ngate fmt ", 1)
        env = {**os.environ, "CXX": "unused"}
        if unset_cxx:
            del env["CXX"]
        result = subprocess.run(
            ["sh", "-c", source, str(SCRIPT), *args],
            env={**env, "PROBE_FAIL": fail, "PROBE_SKIP": skip, "CI": str(ci).lower()},
            capture_output=True,
            text=True,
            check=False,
        )
        gates = [
            line[len("gate:") :]
            for line in result.stdout.splitlines()
            if line.startswith("gate:")
        ]
        return result, gates

    def run_browser_tests(self, root, tier, *, body=None, path=None):
        """Run test.sh's browser_tests on root, then print what it skipped."""
        body = function_body("browser_tests") if body is None else body
        script = (
            f"ROOT={shlex.quote(str(root))}\nskipped=\n"
            f"browser_tests() {{\n{body}\n}}\n"
            f'browser_tests {tier}\nstatus=$?\nprintf "%s" "$skipped"\nexit "$status"\n'
        )
        env = {**os.environ, "CI": "false"}
        if path:
            env["PATH"] = f"{path}{os.pathsep}{env['PATH']}"
        return subprocess.run(
            ["sh", "-c", script],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def reported_skips(self, label, stderr):
        """What test.sh adds to its summary for a command printing stderr."""
        pattern = re.search(
            rf"with_reported_skips {label}\s*\\?\s*'([^']*)'", SCRIPT.read_text()
        )
        self.assertIsNotNone(pattern, label)
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "stderr"
            log.write_text(stderr)
            script = (
                f"skipped=\nwith_reported_skips() {{\n"
                f"{function_body('with_reported_skips')}\n}}\n"
                f"with_reported_skips {label} {shlex.quote(pattern[1])} "
                f"sh -c 'cat \"$1\" >&2' - {shlex.quote(str(log))} 2>/dev/null\n"
                'printf "%s" "$skipped"\n'
            )
            result = subprocess.run(
                ["sh", "-c", script], capture_output=True, text=True, check=True
            )
        return [line.strip() for line in result.stdout.splitlines() if line.strip()]

    def test_parallel_lanes_cover_the_default_gate(self):
        names = list(lanes())
        self.assertEqual(set(names), {"rust", "portable", "browser"})
        self.assertIn("      fail-fast: false\n", gates_job())
        results = {lane: self.run_lane(lane) for lane in ("all", *names)}
        for result, gates in results.values():
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertGreater(len(gates), 0)
            self.assertEqual(len(gates), len(set(gates)))
        full, rust, portable, browser = (set(results[lane][1]) for lane in results)
        shared = {"fetch-vendor", "verify-vendor"}
        self.assertEqual(rust & portable, shared)
        self.assertEqual(
            rust - shared, {"clippy", "cargo-test", "browser-server-tests", "desktop"}
        )
        self.assertEqual(rust & browser, shared)
        self.assertEqual(portable & browser, shared)
        self.assertEqual(browser - shared, {"browser-tests"})
        self.assertEqual(rust | portable | browser | {"cargo-audit"}, full)
        self.assertNotIn("cargo-audit", rust | portable | browser)
        self.assertEqual(self.run_lane()[1], results["all"][1])

    def test_new_gate_is_included_in_portable_lane(self):
        self.assertIn("future-check", self.run_lane("portable", extra_gate=True)[1])
        self.assertNotIn("future-check", self.run_lane("rust", extra_gate=True)[1])

    def test_failure_does_not_hide_later_gates(self):
        for lane, failures in (
            ("rust", "clippy cargo-test"),
            ("portable", "fmt indent"),
        ):
            with self.subTest(lane=lane):
                result, gates = self.run_lane(lane, fail=failures)
                self.assertEqual(result.returncode, 1)
                self.assertIn(f"failed gates: {failures}", result.stderr)
                self.assertIn(
                    "cargo-test" if lane == "rust" else "report-parity-fixtures", gates
                )

    def test_browser_suites_run_once_with_their_build_dependencies(self):
        selected = {}
        with tempfile.TemporaryDirectory() as directory:
            support.write_executable(
                Path(directory) / "node",
                "#!/bin/sh\nfor arg do\n"
                'case "$arg" in --test-timeout=*) ;; '
                '*.test.js) printf "%s\\n" "$arg" >&2 ;; esac\ndone\n',
            )
            for tier in ("server", "static"):
                result = self.run_browser_tests(ROOT, tier, path=directory)
                self.assertEqual(result.returncode, 0, result.stderr)
                selected[tier] = {Path(line) for line in result.stderr.splitlines()}
        suites = set((ROOT / "tests/browser").glob("*.test.js"))
        self.assertEqual(selected["server"] | selected["static"], suites)
        self.assertFalse(selected["server"] & selected["static"])
        self.assertTrue(selected["server"])
        for suite in suites:
            if "target/debug" in suite.read_text():
                self.assertIn(suite, selected["server"], suite.name)
        _, rust = self.run_lane("rust")
        self.assertLess(rust.index("cargo-test"), rust.index("browser-server-tests"))

    def test_ci_refuses_skipped_gates(self):
        for ci in (False, True):
            with self.subTest(ci=ci):
                result, _ = self.run_lane("rust", skip="clippy", ci=ci)
                self.assertEqual(result.returncode, 1 if ci else 0, result.stderr)
                self.assertIn("not checked by this run:", result.stderr)
                if ci:
                    self.assertIn("failed gates: skipped-gates", result.stderr)

    def run_status(self, command, **env):
        passing = dict.fromkeys(("GATES", "AUDIT", "PLAN", "MUTANTS"), "success")
        return subprocess.run(
            ["bash", "-e", "-c", command],
            env={**os.environ, **passing, **env},
            capture_output=True,
            check=False,
        ).returncode

    def test_check_requires_the_lanes_and_explains_every_mutation_skip(self):
        check = support.job("check")
        self.assertIn("    needs: [gates, audit, plan-mutants, mutants]\n", check)
        self.assertIn(f"    if: ${{{{ !cancelled() && {NOT_AVATAR} }}}}\n", check)
        command = support.run_block(check)
        states = ("success", "failure", "cancelled", "skipped", "")
        # The lanes and audit are checked first, so one passing mutation answer
        # stands in for the rest.
        for lane in ("GATES", "AUDIT"):
            for state in states:
                with self.subTest(lane=lane, state=state):
                    status = self.run_status(command, **{lane: state})
                    self.assertEqual(status == 0, state == "success")
        accepted = {
            # Shards ran and passed.
            ("success", "success"),
            # The plan found no mutants, so no shard started.
            ("success", "skipped"),
            # A push or a merge group plans nothing.
            ("skipped", "skipped"),
        }
        for plan in states:
            for mutants in states:
                with self.subTest(plan=plan, mutants=mutants):
                    status = self.run_status(command, PLAN=plan, MUTANTS=mutants)
                    self.assertEqual(status == 0, (plan, mutants) in accepted)

    def test_audit_dispatches_events_and_stops_on_git_or_scan_errors(self):
        audit = support.job("audit")
        command = support.run_block(audit)
        self.assertIn("GROUP_BASE: ${{ github.event.merge_group.base_sha }}", audit)
        self.assertIn("fetch-depth: 2", audit)
        self.assertNotIn("Install cargo-audit", gates_job())
        parent = ["git:rev-parse HEAD^1"]
        group = ["git:cat-file -e group-base^{commit}"]
        fetch = "git:fetch --no-tags --depth=1 origin group-base"
        for event, fail, expected_status, expected_calls in (
            ("push", "", 0, ["python:scripts/audit.py"]),
            (
                "pull_request",
                "",
                0,
                parent + ["python:scripts/audit.py --base actual-parent"],
            ),
            (
                "merge_group",
                "",
                0,
                group + ["python:scripts/audit.py --base group-base"],
            ),
            (
                "merge_group",
                "missing-base",
                0,
                group + [fetch, "python:scripts/audit.py --base group-base"],
            ),
            ("merge_group", "fetch", 1, group + [fetch]),
            ("pull_request", "rev-parse", 1, parent),
            ("push", "scan", 2, ["python:scripts/audit.py"]),
            ("unknown", "", 2, []),
        ):
            with self.subTest(event=event, fail=fail):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    support.write_executable(
                        root / "git",
                        """#!/bin/sh
printf 'git:%s\\n' "$*" >> "$CALLS"
case "$1" in
    rev-parse) [ "$FAIL" != rev-parse ] || exit 1; echo actual-parent ;;
    cat-file) [ "$FAIL" != missing-base ] && [ "$FAIL" != fetch ] ;;
    fetch) [ "$FAIL" != fetch ] ;;
    *) exit 99 ;;
esac
""",
                    )
                    support.write_executable(
                        root / "python3",
                        """#!/bin/sh
printf 'python:%s\\n' "$*" >> "$CALLS"
[ "$FAIL" != scan ] || exit 2
""",
                    )
                    result = subprocess.run(
                        ["bash", "-eo", "pipefail", "-c", command],
                        cwd=root,
                        env={
                            **os.environ,
                            "PATH": str(root) + os.pathsep + os.environ["PATH"],
                            "CALLS": str(root / "calls"),
                            "EVENT": event,
                            "FAIL": fail,
                            "GROUP_BASE": "group-base",
                        },
                        capture_output=True,
                        text=True,
                        timeout=60,
                    )
                    self.assertEqual(result.returncode, expected_status, result.stderr)
                    calls = (
                        (root / "calls").read_text().splitlines()
                        if (root / "calls").exists()
                        else []
                    )
                    self.assertEqual(calls, expected_calls)

    def test_publishing_waits_on_the_lanes_not_on_check(self):
        # A push skips the mutation jobs `check` waits on, and the default
        # `success()` would carry that skip through `check` to both of these.
        self.assertIn("    needs: [gates, audit]\n", support.job("build"))
        self.assertIn("    needs: [gates, audit, build]\n", support.job("release"))

    def test_each_lane_has_a_bounded_job(self):
        timeouts = {lane: int(entry["timeout"]) for lane, entry in lanes().items()}
        self.assertIn("    timeout-minutes: ${{ matrix.timeout }}\n", gates_job())
        self.assertLess(timeouts["browser"], timeouts["rust"])
        self.assertLess(timeouts["portable"], timeouts["rust"])

    def test_browser_timeout_fails_a_stalled_suite(self):
        probe = subprocess.run(
            ["node", "--test-timeout=100", "--version"],
            capture_output=True,
            timeout=10,
            check=False,
        )
        if probe.returncode:
            self.skipTest("this Node version has no test timeout option")
        body = function_body("browser_tests")
        timeout = re.search(r"--test-timeout=(\d+)", body)
        self.assertIsNotNone(timeout)
        self.assertGreater(int(timeout[1]), 0)
        # Two stalled tests must still fail by name before the job is killed.
        browser_minutes = int(lanes()["browser"]["timeout"])
        self.assertLess(2 * int(timeout[1]), browser_minutes * 60 * 1000)
        with tempfile.TemporaryDirectory() as directory:
            tests = Path(directory) / "tests/browser"
            tests.mkdir(parents=True)
            (tests / "stalled.test.js").write_text(
                'const { test } = require("node:test");\n'
                'test("stalled fixture", async () => {\n'
                "  await new Promise(resolve => setTimeout(resolve, 5000));\n"
                "});\n"
            )
            result = self.run_browser_tests(
                directory, "static", body=body.replace(timeout[0], "--test-timeout=100")
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("timed out", result.stdout + result.stderr)

    def test_real_node_skips_and_todos_are_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            tests = Path(directory) / "tests/browser"
            tests.mkdir(parents=True)
            (tests / "mixed.test.js").write_text(
                'const { test } = require("node:test");\n'
                'test("plain pass", () => {});\n'
                'test("a title that says # SKIP is not a skip", () => {});\n'
                'test("no browser", (t) => t.skip("chromium missing"));\n'
                'test("later", { todo: "write it" }, () => {});\n'
            )
            result = self.run_browser_tests(directory, "static")
        self.assertEqual(result.returncode, 0, result.stderr)
        reported = result.stdout.split("\n")[-3:]
        self.assertEqual(
            [line.strip() for line in reported if line.strip()],
            [
                "browser tests, 1 skipped: chromium missing",
                "browser tests, 1 todo: write it",
            ],
        )

    def test_cargo_cache_key_tracks_build_settings(self):
        for tool in ("rustc", "sha256sum"):
            if not shutil.which(tool):
                self.skipTest(f"cache key check needs {tool}")
        script = action_script("restore-cargo-cache")
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "output"
            # One path whose banner changes, as a Clang bump unpacked over the
            # same directory does.
            compiler = Path(tmp) / "clang++"

            def install_clang(revision):
                support.write_executable(
                    compiler, f"#!/bin/sh\necho clang {revision}\n"
                )

            install_clang("1")

            def key(**settings):
                output.write_text("")
                subprocess.run(
                    ["sh", "-eu", "-c", script],
                    env=os.environ
                    | {
                        "GITHUB_OUTPUT": str(output),
                        "RUNNER_OS": "Linux",
                        "INPUTS": "lock",
                        "CXX": str(compiler),
                    }
                    | settings,
                    check=True,
                    capture_output=True,
                )
                return output.read_text()

            baseline = key()
            for name in (
                "CARGO_PROFILE_DEV_DEBUG",
                "CARGO_PROFILE_TEST_DEBUG",
                "CARGO_BUILD_RUSTFLAGS",
                "RUSTFLAGS",
                "RUSTDOCFLAGS",
                "CARGO_INCREMENTAL",
                "CC",
                "CFLAGS",
                "CXXFLAGS",
            ):
                with self.subTest(variable=name):
                    self.assertNotEqual(key(**{name: "changed"}), baseline)
            self.assertEqual(key(UNRELATED_SETTING="changed"), baseline)
            install_clang("2")
            self.assertNotEqual(key(), baseline)
            # A compiler that does not run fails the step rather than leaving
            # a key that silently lost what came after it.
            with self.assertRaises(subprocess.CalledProcessError):
                key(CXX=str(Path(tmp) / "missing-c++"))

    def test_apt_install_asks_only_for_what_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            install = stubbed(
                tmp,
                action_script("apt-install"),
                {
                    # Installed means named in $INSTALLED.
                    "dpkg-query": 'case " $INSTALLED " in *" $3 "*) '
                    "printf installed ;; *) exit 1 ;; esac",
                    "sudo": 'exec "$@"',
                    "dpkg": 'printf "dpkg %s\\n" "$*" >> "$CALLS"',
                    # With $STALE set the first install fails, as a stale list
                    # does; the log it just wrote says whether this is the first.
                    "apt-get": 'shift 4; printf "apt %s\\n" "$*" >> "$CALLS"\n'
                    '[ -n "${STALE:-}" ] && [ "$1" = install ] '
                    '&& [ "$(grep -c "^apt install" "$CALLS")" = 1 ] && exit 100\n'
                    "exit 0",
                },
            )
            self.assertEqual(
                install(PACKAGES="sqlite3 shellcheck", INSTALLED="sqlite3 shellcheck"),
                [],
            )
            self.assertEqual(
                install(PACKAGES="libglib2.0-dev mold sqlite3", INSTALLED="sqlite3"),
                ["apt install -y libglib2.0-dev mold"],
            )
            self.assertEqual(
                install(PACKAGES="mold", INSTALLED="", STALE="1"),
                [
                    "apt install -y mold",
                    "dpkg --configure -a",
                    "apt update",
                    "apt install -y mold",
                ],
            )

    def test_chromium_installs_system_libraries_only_when_one_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            install = stubbed(
                tmp,
                action_script("chromium"),
                {
                    "npx": 'printf "npx %s\\n" "$*" >> "$CALLS"',
                    "node": 'printf "node\\n" >> "$CALLS"',
                    # What the loader prints, found or not.
                    "ldd": 'printf "%s\\n" "$LDD"',
                },
            )
            shell = Path(tmp) / ".cache/ms-playwright/chromium_headless_shell-1"
            shell.mkdir(parents=True)
            # A renamed executable leaves ldd nothing to complain about, so no
            # shell at all has to fail rather than pass as "nothing missing".
            with self.assertRaises(subprocess.CalledProcessError):
                install(HOME=tmp, LDD="")
            (shell / "chrome-headless-shell").write_text("")
            install_shell = "npx playwright install --only-shell chromium"
            self.assertEqual(
                install(HOME=tmp, LDD="\tlibnss3.so => /usr/lib/libnss3.so"),
                [install_shell, "node"],
            )
            self.assertEqual(
                install(HOME=tmp, LDD="\tlibnss3.so => not found"),
                [
                    install_shell,
                    "npx playwright install-deps chromium-headless-shell",
                    "node",
                ],
            )

    def test_every_browser_launch_is_headless(self):
        # The chromium action installs only the headless shell, so a headed or
        # channel launch would find no browser. Say so here, not in CI.
        launchers = [
            *(ROOT / "tests/browser").glob("*.js"),
            *(ROOT / "scripts").glob("*.cjs"),
            *(ROOT / "scripts").glob("*.sh"),
        ]
        for path in launchers:
            with self.subTest(path=path.name):
                self.assertNotRegex(
                    path.read_text(), r"\bheadless:(?!\s*true\b)|\bchannel:"
                )

    def test_avatar_reuses_the_gate_caches_without_saving_them(self):
        avatar = support.job("avatar")
        self.assertIn(
            "    if: github.event_name == 'schedule'"
            " || github.event_name == 'workflow_dispatch'\n",
            avatar,
        )
        # The schedule and the dispatch exist for this job alone.
        gates = gates_job()
        self.assertIn(f"    if: {NOT_AVATAR}\n", gates)
        for name, action in (
            ("Restore cargo cache", "restore-cargo-cache"),
            ("Restore vendored bytes", "restore-vendor"),
            ("Install Chromium", "chromium"),
        ):
            with self.subTest(step=name):
                uses = f"        uses: ./.github/actions/{action}"
                self.assertIn(uses, support.step(avatar, name).splitlines())
                self.assertIn(uses, support.step(gates, name).splitlines())
        self.assertNotIn("actions/cache/save", avatar)
        self.assertNotIn("uses: actions/cache@", avatar)

    def test_only_the_rust_lane_fetches_clang(self):
        if os.uname().sysname != "Linux" or os.uname().machine != "x86_64":
            self.skipTest("the pinned Clang is only fetched on Linux x86_64")
        fetch = 'sh "$ROOT/scripts/fetch-clang.sh" "$ROOT/target/clang"'
        self.assertEqual(SCRIPT.read_text().count(fetch), 1)
        for lane, fetched in (("all", True), ("rust", True), ("portable", False)):
            with self.subTest(lane=lane):
                result, _ = self.run_lane(
                    lane, unset_cxx=True, replace=(fetch, "{ echo fetch:clang >&2; }")
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual("fetch:clang" in result.stderr, fetched)

    def test_each_lane_installs_what_its_gates_need(self):
        gates, matrix = gates_job(), lanes()
        self.assertEqual(condition(gates, "Install system packages"), "matrix.packages")
        self.assertEqual(condition(gates, "Install Rust"), "matrix.components")
        self.assertEqual(matrix["rust"]["components"], "clippy")
        self.assertEqual(matrix["portable"]["components"], "rustfmt")
        self.assertTrue({"mold", "sqlite3"} <= set(matrix["rust"]["packages"].split()))
        self.assertEqual(
            set(matrix["portable"]["packages"].split()), {"shellcheck", "shfmt"}
        )
        self.assertEqual(set(matrix["browser"]), {"lane", "timeout"})
        for name in ("ruff", "commentflow", "actionlint"):
            with self.subTest(tool=name):
                self.assertEqual(
                    condition(gates, f"Install {name}"), "matrix.lane == 'portable'"
                )
        self.assertEqual(
            condition(gates, "Install Chromium"), "matrix.lane != 'portable'"
        )
        # After the gates pass, so the binary is this checkout's.
        for name in (
            "Prove the build names its commit",
            "Run the browser interview check",
        ):
            self.assertEqual(condition(gates, name), "matrix.lane == 'rust'")
        # Every lane restores the vendor tree and Chromium; one lane saves each,
        # after its gates, under the key the restores asked for.
        self.assertEqual(condition(gates, "Restore vendored bytes"), "")
        for name, lane, step_id in (
            ("Save vendored bytes", "portable", "vendor"),
            ("Save Chromium", "browser", "chromium"),
        ):
            with self.subTest(step=name):
                self.assertEqual(
                    condition(gates, name),
                    f"github.event_name != 'merge_group' && matrix.lane == '{lane}' && "
                    f"steps.{step_id}.outputs.cache-hit != 'true'",
                )
                self.assertIn(
                    f"key: ${{{{ steps.{step_id}.outputs.key }}}}",
                    support.step(gates, name),
                )
                self.assertLess(
                    gates.index("- name: Run gates"), gates.index(f"- name: {name}")
                )
        # GitHub reads 0 as false, so `portable && 0 || 1` would always be 1.
        self.assertIn(
            "fetch-depth: ${{ matrix.lane != 'portable' && 1 || 0 }}\n", gates
        )

    def test_formatter_skips_join_the_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / "repo"
            (work / "scripts").mkdir(parents=True)
            shutil.copy(ROOT / "scripts/indent.sh", work / "scripts/indent.sh")
            (work / "example.sh").write_text("#!/bin/sh\necho ok\n")
            (work / "Cargo.toml").write_text("")
            (work / ".editorconfig").write_text("")
            subprocess.run(["git", "init", "-q"], cwd=work, check=True)
            # Only the plumbing indent.sh needs, so no formatter is on PATH.
            tools = Path(directory) / "bin"
            tools.mkdir()
            for tool in (
                "git",
                "sort",
                "tar",
                "mktemp",
                "cat",
                "cmp",
                "grep",
                "rm",
                "dirname",
                "tr",
            ):
                found = shutil.which(tool)
                self.assertIsNotNone(found, tool)
                (tools / tool).symlink_to(found)

            def indent(*flags):
                return subprocess.run(
                    [shutil.which("sh"), "scripts/indent.sh", *flags],
                    cwd=work,
                    env={"PATH": str(tools), "HOME": directory},
                    capture_output=True,
                    text=True,
                    timeout=60,
                    check=False,
                )

            result = indent("--check", "--skip-missing")
            # Outside the gate a missing formatter is a failure, not a pass
            # with a footnote.
            strict = indent("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "formatting, commentflow not installed",
            self.reported_skips("formatting", result.stderr),
        )
        self.assertIn("not checked: commentflow shfmt.", result.stdout)
        self.assertEqual(strict.returncode, 2, strict.stderr)
        self.assertIn("commentflow: install", strict.stderr)

    def test_python_skips_join_the_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            suite = Path(directory) / "test_skips.py"
            suite.write_text(
                "import unittest\n"
                "class Cases(unittest.TestCase):\n"
                "    def test_runs(self):\n"
                "        pass\n"
                "    def test_skips(self):\n"
                "        self.skipTest('tool missing')\n"
            )
            result = subprocess.run(
                [sys.executable, str(ROOT / "scripts/run-python-tests.py"), str(suite)],
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        reported = self.reported_skips('"python tests"', result.stderr)
        self.assertEqual(len(reported), 1, reported)
        self.assertRegex(reported[0], r"^python tests, test_skips .*: tool missing$")

    def test_unknown_lane_fails_before_running_any_gate(self):
        result, gates = self.run_lane("typo")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(gates, [])
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
