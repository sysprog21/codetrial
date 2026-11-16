"""The shape of the mutation job and the aggregate status in check.yml."""

import importlib.util
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "check_workflow_support", ROOT / "tests/support.py"
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)


def header(body, key):
    """The value of a job-level key, with a folded scalar joined to one line."""
    found = re.search(rf"^    {key}: (.*(?:\n      .*)*)", body, re.M)
    if found is None:
        raise AssertionError(f"job no longer sets {key}")
    return " ".join(found[1].replace(">-", "").split())


def needs_of(body):
    return set(re.findall(r"[a-z][a-z-]*", header(body, "needs")))


class MutantsJobTests(unittest.TestCase):
    def setUp(self):
        self.body = support.job("mutants")

    def test_waits_for_the_gates_and_the_plan(self):
        needs = needs_of(self.body)
        self.assertLessEqual({"gates", "plan-mutants"}, needs)

    def test_starts_only_after_both_succeed_with_work_to_do(self):
        condition = header(self.body, "if").strip("${} ")
        # Exactly these terms, all required. `!cancelled()` replaces the
        # implicit success() that would otherwise skip the job. plan-mutants
        # is skipped on push and merge_group, where its count output is empty
        # and would pass the != '0' guard on its own.
        self.assertEqual(
            [
                "!cancelled()",
                "needs.plan-mutants.result == 'success'",
                "needs.gates.result == 'success'",
                "needs.plan-mutants.outputs.count != '0'",
            ],
            [term.strip() for term in condition.split("&&")],
        )


def mutants_invocations(body):
    """The argument words of every `cargo mutants` command.

    Continuation lines are joined, comments dropped, and the words end at the
    first redirection, pipe or `||`, so a flag only counts when the shell would
    pass it.
    """
    found = []
    for command in re.findall(
        r"^[^#\n]*?cargo mutants\b((?:[^\n]*\\\n)*[^\n]*)", body, re.M
    ):
        words = []
        for word in shlex.split(command.replace("\\\n", " "), comments=True):
            if word[0] in "|>":
                break
            words.append(word)
        found.append(words)
    return found


class MutantsLockfileTests(unittest.TestCase):
    def setUp(self):
        self.plan = mutants_invocations(support.job("plan-mutants"))
        self.shard = mutants_invocations(support.job("mutants"))

    def test_every_selecting_or_running_command_holds_the_lockfile(self):
        # cargo-mutants forwards the flag to every cargo it runs; without it
        # a mutant can be judged against a lockfile cargo quietly updated.
        commands = self.plan + self.shard
        self.assertEqual(3, len(commands))
        for words in commands:
            self.assertIn("--cargo-arg=--locked", words, words)

    def test_plan_and_shard_select_the_same_mutants(self):
        # The shard count comes from the plan's list, so what is listed has to
        # match on both sides: the same flags with the same values.
        def selection(words):
            kept, skip = [], False
            for word in words:
                if skip:
                    skip = False
                elif word == "--shard":
                    skip = True
                elif word not in ("--list", "--in-place"):
                    kept.append(word)
            return sorted(kept)

        listings = [w for w in self.plan + self.shard if "--list" in w]
        self.assertEqual(2, len(listings))
        self.assertEqual(selection(listings[0]), selection(listings[1]))
        run = next(w for w in self.shard if "--list" not in w)
        self.assertEqual(selection(listings[1]), selection(run))


class MutantsLocalRunDocsTests(unittest.TestCase):
    """docs/development.md tells a contributor to run what CI's shard step runs."""

    def setUp(self):
        self.text = (support.ROOT / "docs/development.md").read_text()
        self.block = support.section(
            self.text, "```sh\nSCRATCH=", "```\n", "the local mutation commands"
        )

    def test_both_documented_commands_hold_the_lockfile_like_ci(self):
        documented = mutants_invocations(self.block)
        self.assertEqual(2, len(documented))
        for words in documented:
            self.assertIn("--cargo-arg=--locked", words, words)

    def test_the_run_selects_what_the_shard_step_selects(self):
        # Only the diff's location and the shard differ between a local run and
        # a shard of CI's plan.
        def selection(words):
            kept, skip = [], False
            for word in words:
                if skip:
                    skip = False
                elif word in ("--in-diff", "--shard"):
                    skip = True
                elif word != "--list":
                    kept.append(word)
            return sorted(kept)

        run = next(w for w in mutants_invocations(self.block) if "--list" not in w)
        shard = next(
            w for w in mutants_invocations(support.job("mutants")) if "--list" not in w
        )
        self.assertEqual(selection(shard), selection(run))


class AggregateStatusTests(unittest.TestCase):
    def test_check_accepts_mutants_skipped_after_a_plan(self):
        body = support.job("check")
        self.assertRegex(body, r"success/success \| skipped/success[^)]*\) ;;")
        # A failed gates lane fails the job before the mutants table is read.
        self.assertLess(body.index('"$GATES" != success'), body.index("case "))

    def test_release_does_not_wait_for_mutants(self):
        self.assertNotIn("mutants", header(support.job("release"), "needs"))


def steps_text(body):
    """What follows `steps:`, so the job's own keys are not read as a step's."""
    return support.section(body, "\n    steps:\n", "\0", "steps")


def run_blocks(body):
    """Every `run: |` script of a job, dedented as the shell reads it."""
    blocks = []
    key = re.compile(r"^ *run: \|\n", re.M)
    rest = body
    while (found := key.search(rest)) is not None:
        blocks.append(support.run_block(rest))
        rest = rest[found.end() :]
    return blocks


# What the base branch must hold for preflight to judge anything.
GATE_FILES = {
    "gatherer": ".claude/skills/codetrial-triage/gate-evidence.sh",
    "checker": "scripts/check-gate-evidence.py",
    "fetcher": "scripts/fetch-gate-note.sh",
}


class PreflightJobTests(unittest.TestCase):
    def setUp(self):
        self.body = support.job("preflight")

    def test_runs_only_for_a_pull_request(self):
        self.assertEqual("github.event_name == 'pull_request'", header(self.body, "if"))

    def test_is_bounded_and_read_only(self):
        self.assertEqual("ubuntu-24.04", header(self.body, "runs-on"))
        self.assertEqual("5", header(self.body, "timeout-minutes"))
        perms = support.section(
            self.body, "    permissions:\n", "\n    steps:", "permissions"
        )
        # pull-requests: the pull request and its file list come from the pulls
        # API, which a private repository serves only to this scope.
        self.assertEqual(
            {"contents": "read", "pull-requests": "read"},
            dict(re.findall(r"^      ([a-z-]+): (\S+)$", perms, re.M)),
        )

    def test_checks_out_the_base_and_never_the_head(self):
        steps = steps_text(self.body)
        self.assertEqual(1, steps.count("actions/checkout@"))
        checkout = support.section(steps, "actions/checkout@", "\n      - ", "checkout")
        self.assertIn("ref: ${{ github.event.pull_request.base.sha }}", checkout)
        self.assertIn("persist-credentials: false", checkout)
        self.assertIn("path: base", checkout)
        self.assertNotIn("head", checkout)
        # No other action runs, and every script a run line starts is the base's.
        self.assertEqual(
            ["actions/checkout"],
            [u.split("@")[0] for u in re.findall(r"uses: (\S+)", self.body)],
        )
        for block in run_blocks(self.body):
            for script in re.findall(
                r"\b(?:sh|python3\.12|bash) +(\S+\.(?:sh|py))\b", block
            ):
                self.assertTrue(script.startswith("base/"), script)

    def test_event_data_reaches_the_script_only_through_env(self):
        for block in run_blocks(self.body):
            self.assertNotIn("${{", block)
        # Every expression is an `env:` value or the checkout's `ref:`.
        for line in steps_text(self.body).splitlines():
            if "${{" in line:
                self.assertRegex(
                    line, r"^ {10}(?:[A-Z_]+|ref): \$\{\{ [^}]+\}\}$", line
                )

    def test_pull_request_text_cannot_leave_the_job(self):
        self.assertNotIn("\n    outputs:", self.body)
        for name in ("GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_PATH", "GITHUB_STATE"):
            self.assertNotIn(name, self.body)
        env = support.section(self.body, "        env:\n", "        run:", "env")
        # What it hands the script is a number, a sha and the repository name:
        # not the title, body, branch name or login, which a pull request writes.
        for field in ("title", "body", "head.ref", "label", "user.login"):
            self.assertNotIn(f"pull_request.{field}", env)
        self.assertIn("github.event.pull_request.head.sha", env)

    def test_judges_with_the_base_scripts_and_names_the_interpreter(self):
        block = run_blocks(self.body)[0]
        for path in GATE_FILES.values():
            self.assertIn(f"base/{path}", block)
        self.assertIn("GATE_EVIDENCE_PYTHON: python3.12", self.body)

    def run_step(self, present, status=0, reason="the reason", out=None, err=""):
        """Run the step's script over a base checkout holding `present` files.

        The stub gatherer prints `out` (a verdict naming `reason` when None) on
        stdout and `err` on stderr, then exits `status`.
        """
        block = run_blocks(self.body)[0]
        with tempfile.TemporaryDirectory() as work:
            for key in present:
                path = Path(work, "base", GATE_FILES[key])
                path.parent.mkdir(parents=True, exist_ok=True)
                said = '{"reason": "%s"}' % reason if out is None else out
                body = (
                    f"#!/bin/sh\ncat <<'__OUT__'\n{said}\n__OUT__\n"
                    f"cat >&2 <<'__ERR__'\n{err}\n__ERR__\nexit {status}\n"
                )
                path.write_text(body if key == "gatherer" else "")
            return subprocess.run(
                # dash where there is one: its `echo` expands escapes, so the
                # step must not rely on a shell that does not.
                [shutil.which("dash") or "sh", "-c", block],
                cwd=work,
                env=dict(
                    os.environ, REPOSITORY="o/r", PULL_NUMBER="1", HEAD_SHA="a" * 40
                ),
                capture_output=True,
                text=True,
            )

    def test_a_base_without_the_check_passes_with_one_line(self):
        done = self.run_step([])
        self.assertEqual(0, done.returncode)
        self.assertEqual(1, len(done.stdout.splitlines()))

    def test_a_base_with_part_of_the_check_fails_closed(self):
        for present in (["gatherer"], ["checker", "fetcher"], ["gatherer", "checker"]):
            self.assertEqual(1, self.run_step(present).returncode, present)

    def test_each_verdict_maps_to_an_exit_and_a_message(self):
        everything = ["gatherer", "checker", "fetcher"]
        self.assertEqual(0, self.run_step(everything, 0).returncode)
        asked = self.run_step(everything, 1)
        self.assertEqual(1, asked.returncode)
        self.assertIn("./scripts/test.sh", asked.stdout)
        self.assertIn("the reason", asked.stdout)
        # An unjudgeable head and a gatherer that died without a verdict fail.
        for status in (2, 127):
            closed = self.run_step(everything, status, reason="")
            self.assertEqual(1, closed.returncode, status)
            self.assertIn("fails closed", closed.stdout)

    def test_no_gatherer_output_can_forge_a_workflow_command(self):
        # The gatherer reads data a pull request writes, so whatever it prints,
        # on either stream and on any exit, must not put a `::` command in the
        # log: the one `::error::` line is the step's own.
        everything = ["gatherer", "checker", "fetcher"]
        newline = r'{"reason": "no receipt\n::add-mask::topsecret\n::error::forged"}'
        garbage = "::add-mask::topsecret\nx\n::error::forged\r::stop-commands::k"
        control = r'{"reason": "tab\u0007bell\r::error::x"}'
        long = '{"reason": "%s"}' % ("a" * 1000)
        for status in (0, 1, 2, 127):
            for out, err in (
                (newline, ""),
                (garbage, garbage),
                (control, garbage),
                # A literal backslash-n, which an `echo` that expands escapes
                # (dash's) would turn into a line break.
                (r'{"reason": "x\\n::stop-commands::k\\r"}', ""),
                # No usable verdict, so the first stderr line is the reason.
                ("not json", "x\r::add-mask::topsecret\n::error::forged"),
                (long, ""),
            ):
                with self.subTest(status=status, out=out[:20]):
                    done = self.run_step(everything, status, out=out, err=err)
                    log = (done.stdout + done.stderr).replace("\r", "\n")
                    commands = [x for x in log.splitlines() if x.startswith("::")]
                    self.assertEqual(0 if status == 0 else 1, len(commands), commands)
                    for line in commands:
                        self.assertTrue(line.startswith("::error::gate evidence"), line)
                        self.assertLessEqual(len(line), 400)
                    self.assertTrue(log.replace("\n", "").isprintable(), log)

    def test_repeats_the_checkers_install_steps(self):
        # The shared gatherer keeps one line of the checker's message, so the
        # step says the steps itself; they must stay the checker's words.
        source = (ROOT / "scripts/check-gate-evidence.py").read_text()
        guidance = re.search(r'GUIDANCE = \(\n    "([^"]+)"', source)
        self.assertIn(guidance[1], self.body)

    def test_only_the_description_workflow_uses_a_target_trigger(self):
        found = sorted(
            path.name
            for path in (ROOT / ".github/workflows").glob("*.yml")
            if re.search(
                r"^\s*(pull_request_target|workflow_run):", path.read_text(), re.M
            )
        )
        self.assertEqual(["pr-description.yml"], found)


class PreflightWiringTests(unittest.TestCase):
    def test_gates_wait_for_it_and_keep_the_skipped_arm(self):
        body = support.job("gates")
        self.assertIn("preflight", needs_of(body))
        self.assertEqual(
            [
                "!cancelled()",
                "(needs.preflight.result == 'success' || needs.preflight.result == 'skipped')",
                "github.event_name != 'schedule'",
                "github.event_name != 'workflow_dispatch'",
            ],
            [t.strip() for t in header(body, "if").strip("${} ").split("&&")],
        )

    def test_the_plan_waits_for_it_without_cancelled(self):
        plan = support.job("plan-mutants")
        self.assertIn("preflight", needs_of(plan))
        self.assertNotIn("cancelled()", header(plan, "if"))
        self.assertNotIn("always()", header(plan, "if"))

    def test_the_aggregate_lists_it(self):
        body = support.job("check")
        self.assertIn("preflight", needs_of(body))
        self.assertIn("| preflight |", body)
        self.assertIn("PREFLIGHT: ${{ needs.preflight.result }}", body)

    def test_release_and_build_do_not_wait_for_it(self):
        self.assertNotIn("preflight", needs_of(support.job("release")))
        self.assertNotIn("\n    needs:", support.job("build"))


def evaluate(condition, event, results, cancelled=False, outputs=None):
    """A job condition over an event and the results of the jobs it needs.

    Enough of GitHub's expression language for what this workflow writes. It
    shows the text means what the comments say for the cases they name; it does
    not show that GitHub reads a skipped dependency, an empty output or
    `fromJson` the same way.
    """
    text = condition.strip().removeprefix("${{").removesuffix("}}").strip()
    text = re.sub(r"needs\.([a-z-]+)\.result", lambda m: f"R[{m[1]!r}]", text)
    text = re.sub(
        r"needs\.([a-z-]+)\.outputs\.(\w+)", lambda m: f"O[{m[1] + '.' + m[2]!r}]", text
    )
    text = text.replace("github.event_name", "E")
    text = re.sub(r"github\.event\.pull_request\.[a-z_.]+", "'same'", text)
    text = text.replace("github.repository", "'same'")
    text = text.replace("!cancelled()", "(not C)").replace("always()", "True")
    text = re.sub(r"!(?!=)", "not ", text)
    text = text.replace("&&", " and ").replace("||", " or ")
    scope = {"E": event, "R": results, "O": outputs or {}, "C": cancelled}
    return bool(eval(text, {"__builtins__": {}}, scope))


def simulate(event, preflight_fails=False):
    """Each gating job's result, assuming a job that starts passes."""
    results = {}
    outputs = {"plan-mutants.count": "3"}
    for name in ("preflight", "gates", "plan-mutants", "mutants"):
        body = "\n" + support.job(name)
        condition = header(body, "if") if "\n    if:" in body else "True"
        runs = evaluate(condition, event, results, outputs=outputs)
        if not re.search(r"\b(always|cancelled|failure|success)\(\)", condition):
            # No status function: GitHub adds success(), which a skipped or
            # failed dependency fails.
            needed = needs_of(body) if "\n    needs:" in body else set()
            runs = runs and all(results[n] == "success" for n in needed)
        failed = name == "preflight" and preflight_fails
        results[name] = "skipped" if not runs else "failure" if failed else "success"
    return results


class ConditionModelTests(unittest.TestCase):
    def test_a_pull_request_runs_everything_when_the_evidence_holds(self):
        self.assertEqual(
            {
                "preflight": "success",
                "gates": "success",
                "plan-mutants": "success",
                "mutants": "success",
            },
            simulate("pull_request"),
        )

    def test_a_failed_preflight_starts_nothing_else(self):
        self.assertEqual(
            {
                "preflight": "failure",
                "gates": "skipped",
                "plan-mutants": "skipped",
                "mutants": "skipped",
            },
            simulate("pull_request", preflight_fails=True),
        )

    def test_push_and_merge_group_still_run_the_gates(self):
        for event in ("push", "merge_group"):
            results = simulate(event)
            self.assertEqual("skipped", results["preflight"], event)
            self.assertEqual("success", results["gates"], event)
            self.assertEqual("skipped", results["mutants"], event)

    def test_a_schedule_runs_no_gates(self):
        self.assertEqual("skipped", simulate("schedule")["gates"])

    def test_a_cancelled_run_starts_no_gates(self):
        gates = header(support.job("gates"), "if")
        self.assertFalse(
            evaluate(gates, "pull_request", {"preflight": "success"}, cancelled=True)
        )


class AggregatePreflightTests(unittest.TestCase):
    """Run the aggregate job's own script over preflight results."""

    def run_check(self, preflight, gates="success"):
        script = run_blocks(support.job("check"))[0]
        env = dict(
            os.environ,
            GATES=gates,
            AUDIT="success",
            PLAN="skipped",
            MUTANTS="skipped",
            PREFLIGHT=preflight,
            GITHUB_STEP_SUMMARY=os.devnull,
        )
        return subprocess.run(
            ["sh", "-c", script], env=env, capture_output=True, text=True
        )

    def test_success_and_skipped_pass(self):
        for result in ("success", "skipped"):
            self.assertEqual(0, self.run_check(result).returncode, result)

    def test_a_failed_preflight_names_the_evidence_check(self):
        # The gates are skipped behind it, so its message has to come first.
        done = self.run_check("failure", gates="skipped")
        self.assertEqual(1, done.returncode)
        self.assertIn("evidence", done.stdout.lower())

    def test_a_cancelled_preflight_fails(self):
        self.assertEqual(1, self.run_check("cancelled", gates="skipped").returncode)


if __name__ == "__main__":
    unittest.main()
