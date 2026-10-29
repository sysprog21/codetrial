#!/usr/bin/env python3

"""Completion gate for the recovery cancellation coverage ledger.

The ledger is a JSON list of rows, one per (phase, cause) pair of the closed
recovery matrix: 29 phases crossed with six cause classes, 174 rows. It lives
outside the checkout (it is working evidence, not a source), so every command
takes its path explicitly. A row is one of:

  covered_local_terminal_cleanup  a local test proves the cleanup
  covered_shared                  another row's test covers it, for a reason
  N/A                             the pair cannot occur, for a source reason
  uncovered                       open work

`check` resolves each tested row to a fully qualified test that exists in the
`cargo test --lib -- --list` output and to a receipt from a successful run that
discovered at least one test against the current sources, and resolves each
row's SDK column to an executed run-budget result (scripts/interview-run-budget.py
writes budget-result.json and run-manifest.json). Two modes:

  incremental  reports gaps (uncovered rows, missing tests or receipts, stale
               or zero-test receipts, SDK rows not yet executed) and exits 0.
  strict       exits 1 on any gap. It is the completion gate: the full ledger
               passes only once every SDK row names an executed run.

Ledger lies fail in both modes, because no amount of further work repairs a
ledger that misstates itself: duplicate or unexpected rows, rows dropped from
the baseline, an unknown cause class, N/A or covered_shared without a source
reason, a row whose test field names several tests without an explicit `tests`
list, and SDK coverage that no executed run backs.

Receipts are plain JSON bound to the source digest, so they are evidence of a
recorded run, not proof against someone who writes one by hand; the gate defends
against stale, empty and mismatched receipts, not forgery. Likewise an SDK claim
is checked against the executed result (granted action, completed run, matching
manifest), and which scenario an action drove is the live run's to record.

`record` runs each resolved test with `cargo test --lib <test> -- --exact` and
writes the receipts file. `digest` prints the source digest receipts bind to.

Exit codes: 0 pass, 1 gate failure, 2 unusable input.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

CAUSES = ("End", "Deadline", "RoomDisconnected", "Absent", "Finish", "RoomEventsEof")
STATUSES = ("covered_local_terminal_cleanup", "covered_shared", "N/A", "uncovered")
SDK_PENDING = "uncovered required T4b.4"
RECEIPT_SCHEMA = 1

# The closed baseline: 29 phases x 6 causes. A ledger with any other row count,
# or whose (phase, cause) set hashes differently, has silently dropped or
# invented a row. Synthetic ledgers pass their own baseline with --synthetic-baseline
# (the real gate omits it).
BASELINE_ROWS = 174
BASELINE_PHASES = 29
BASELINE_KEYS_SHA256 = (
    "c20b2dcd03123dda042727dfd73cac19da94615c4bd8e4a7b5412fe84601c205"
)

SOURCE_GLOBS = (
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "config/**/*",
    "vendor/**/*",
    "web/**/*",
    "src/**/*.rs",
    "tests/**/*.rs",
)
IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*$")
SOURCE_ANCHOR = re.compile(r"\.rs\b|\bsrc/|\btests/|\b[a-z]+_[a-z_]+\b|::|\w\.\w")
PLACEHOLDERS = {"n/a", "na", "none", "todo", "tbd", "-", "?"}
MIN_REASON = 40


class Unusable(Exception):
    """Input that cannot be read at all; exit code 2."""


def sha256_hex(data):
    return hashlib.sha256(data).hexdigest()


def row_id(row):
    return "%s/%s" % (row.get("phase"), row.get("cause"))


def keys_digest(keys):
    text = "".join("%s/%s\n" % key for key in sorted(set(keys)))
    return sha256_hex(text.encode())


def source_digest(root):
    """Digest of every file a unit test result can depend on, by relative path."""
    root = Path(root)
    files = set()
    for pattern in SOURCE_GLOBS:
        files.update(path for path in root.glob(pattern) if path.is_file())
    digest = hashlib.sha256()
    for path in sorted(files):
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(sha256_hex(path.read_bytes()).encode() + b"\n")
    return digest.hexdigest()


def read_json(path, what):
    try:
        with open(path, "rb") as handle:
            return json.loads(handle.read().decode("utf-8"))
    except (OSError, ValueError) as error:
        raise Unusable("cannot read %s %s: %s" % (what, path, error))


def load_baseline(path):
    """(rows, phases, keys digest) the ledger must match."""
    if path is None:
        return BASELINE_ROWS, BASELINE_PHASES, BASELINE_KEYS_SHA256
    data = read_json(path, "baseline")
    keys = []
    if not isinstance(data, list):
        raise Unusable("baseline %s must be a JSON list of [phase, cause]" % path)
    for item in data:
        if isinstance(item, dict):
            item = [item.get("phase"), item.get("cause")]
        if not (
            isinstance(item, list)
            and len(item) == 2
            and all(isinstance(v, str) for v in item)
        ):
            raise Unusable("baseline %s has a malformed key" % path)
        keys.append(tuple(item))
    return len(set(keys)), len({k[0] for k in keys}), keys_digest(keys)


def parse_test_list(text):
    """Fully qualified test names from `cargo test -- --list` output."""
    names = []
    for line in text.splitlines():
        line = line.strip()
        if line.endswith(": test"):
            names.append(line[: -len(": test")])
    return names


def cargo_test_list(cargo, root):
    command = [cargo, "test", "--locked", "--lib", "--", "--list"]
    try:
        done = subprocess.run(
            command,
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
    except OSError as error:
        raise Unusable("cannot run %s: %s" % (command[0], error))
    if done.returncode != 0:
        raise Unusable(
            "%s failed (exit %d): %s"
            % (" ".join(command), done.returncode, done.stderr[-400:])
        )
    return parse_test_list(done.stdout)


def resolve_name(name, tests):
    """(fully qualified name, None) or (None, reason)."""
    if name in tests:
        return name, None
    matches = sorted(t for t in tests if t.endswith("::" + name))
    if not matches:
        return None, "no test named %s in the test list" % name
    if len(matches) > 1:
        return None, "%s is ambiguous: %s" % (name, ", ".join(matches))
    return matches[0], None


class Report:
    def __init__(self):
        self.integrity = []  # (row id, kind, detail): the ledger misstates itself
        self.gaps = []  # (row id, kind, detail): work remains
        self.rows = 0
        self.by_status = {}

    def lie(self, rid, kind, detail):
        self.integrity.append((rid, kind, detail))

    def gap(self, rid, kind, detail):
        self.gaps.append((rid, kind, detail))


def check_structure(rows, baseline, report):
    """Row shape, unique ids, causes and baseline membership. Returns valid rows."""
    want_rows, want_phases, want_digest = baseline
    if not isinstance(rows, list):
        raise Unusable("ledger must be a JSON list of rows")
    valid, seen = [], {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            report.lie("row %d" % index, "malformed", "row is not an object")
            continue
        rid = row_id(row)
        ok = True
        for field in ("phase", "cause", "status", "test", "reason"):
            if not isinstance(row.get(field), str):
                report.lie(rid, "malformed", "%s must be a string" % field)
                ok = False
        if ok and "/" in row["phase"]:
            report.lie(rid, "malformed", "phase names cannot contain '/'")
            ok = False
        if not ok:
            continue
        if row["cause"] not in CAUSES:
            report.lie(
                rid,
                "unknown_cause",
                "%r is not one of %s" % (row["cause"], ", ".join(CAUSES)),
            )
            ok = False
        if row["status"] not in STATUSES:
            report.lie(
                rid,
                "bad_status",
                "%r is not one of %s" % (row["status"], ", ".join(STATUSES)),
            )
            ok = False
        if rid in seen:
            report.lie(rid, "duplicate", "also row %d" % seen[rid])
            continue
        seen[rid] = index
        if ok:
            valid.append(row)
    keys = [tuple(rid.split("/", 1)) for rid in seen]
    if len(seen) != want_rows or len({k[0] for k in keys}) != want_phases:
        report.lie(
            "ledger",
            "dropped_or_extra",
            "%d unique rows over %d phases; the baseline has %d over %d"
            % (len(seen), len({k[0] for k in keys}), want_rows, want_phases),
        )
    if keys_digest(keys) != want_digest:
        report.lie(
            "ledger",
            "baseline_mismatch",
            "the (phase, cause) set differs from the pinned baseline",
        )
    return valid


def check_reason(row, report):
    rid, status, reason = row_id(row), row["status"], row["reason"].strip()
    if status not in ("N/A", "covered_shared"):
        return
    if reason.lower() in PLACEHOLDERS or len(reason) < MIN_REASON:
        report.lie(
            rid,
            "missing_reason",
            "%s needs a source reason of at least %d characters" % (status, MIN_REASON),
        )
    elif status == "N/A" and not SOURCE_ANCHOR.search(reason):
        report.lie(
            rid, "missing_reason", "N/A reason names no source path or identifier"
        )


def row_test_names(row, report):
    """Test names a row relies on; multi-test rows must list them explicitly."""
    rid, field, listed = row_id(row), row["test"].strip(), row.get("tests")
    if listed is not None:
        if not (
            isinstance(listed, list)
            and listed
            and all(isinstance(n, str) and IDENT.match(n) for n in listed)
        ):
            report.lie(rid, "multi_test", "tests must be a nonempty list of test names")
            return []
        if IDENT.match(field) and field not in listed:
            report.lie(rid, "multi_test", "test %s is not in the tests list" % field)
            return []
        # A field written as a comma list of test names must be fully listed.
        pieces = [p.strip() for p in re.split(r"[,;]", field) if p.strip()]
        if len(pieces) > 1 and all(IDENT.match(p) for p in pieces):
            unlisted = [p for p in pieces if p not in listed]
            if unlisted:
                report.lie(
                    rid,
                    "multi_test",
                    "test field names %s, absent from the tests list"
                    % ", ".join(unlisted),
                )
                return []
        return list(dict.fromkeys(listed))
    if IDENT.match(field):
        return [field]
    if row["status"] == "N/A" and not field:
        return []
    report.lie(
        rid,
        "multi_test",
        "test field %r names no single test; add an explicit tests list" % field,
    )
    return []


def is_count(value, exact=None):
    if type(value) is not int:
        return False
    return value == exact if exact is not None else value > 0


def receipt_problem(receipt, name, digest):
    if type(receipt.get("exit")) is not int or receipt["exit"] != 0:
        return "failed_receipt", "run exited %r" % (receipt.get("exit"),)
    discovered, passed = receipt.get("discovered"), receipt.get("passed")
    if not is_count(discovered) or not is_count(passed):
        return "zero_test", "success with %r discovered and %r passed" % (
            discovered,
            passed,
        )
    if discovered != 1 or passed != 1:
        # `--exact` on one full name selects exactly one test.
        return "failed_receipt", "%d discovered and %d passed; --exact runs one" % (
            discovered,
            passed,
        )
    command = receipt.get("command")
    if not (
        isinstance(command, list)
        and command[-3:] == [name, "--", "--exact"]
        and command[1:2] == ["test"]
    ):
        return "failed_receipt", "command is not `cargo test ... %s -- --exact`" % name
    if receipt.get("source_digest") != digest:
        return "stale_receipt", "receipt digest does not match the current sources"
    return None


def check_local(row, names, tests, receipts, digest, report):
    rid = row_id(row)
    for name in names:
        full, why = resolve_name(name, tests)
        if full is None:
            report.gap(rid, "missing_test", why)
            continue
        found = [r for r in receipts if isinstance(r, dict) and r.get("test") == full]
        if not found:
            report.gap(rid, "no_receipt", "no receipt for %s" % full)
            continue
        # The last receipt listed for a test decides, so a file that appends
        # a later failing run after a pass is judged by the failure.
        problem = receipt_problem(found[-1], full, digest)
        if problem:
            report.gap(rid, problem[0], "%s: %s" % (full, problem[1]))


def load_sdk_runs(directory):
    """{run id: (result, manifest bytes digest or None)} for each budget-result.json."""
    runs = {}
    if directory is None:
        return runs
    for path in sorted(Path(directory).glob("**/budget-result.json")):
        try:
            result = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if not isinstance(result, dict) or not isinstance(result.get("run_id"), str):
            continue
        manifest = path.with_name("run-manifest.json")
        digest = sha256_hex(manifest.read_bytes()) if manifest.is_file() else None
        if result["run_id"] in runs:
            raise Unusable(
                "run %s has more than one budget-result.json under %s"
                % (result["run_id"], directory)
            )
        runs[result["run_id"]] = (result, digest)
    return runs


def sdk_problem(claim, runs):
    """None when the claimed run executed the claimed action, else the reason."""
    if not (isinstance(claim, dict) and set(claim) == {"run_id", "action_id"}):
        return "sdk_report must be an object with run_id and action_id"
    if not all(isinstance(claim[k], str) and claim[k] for k in claim):
        return "sdk_report run_id and action_id must be nonempty strings"
    if claim["run_id"] not in runs:
        return "no budget-result.json for run %s" % claim["run_id"]
    result, manifest_digest = runs[claim["run_id"]]
    if manifest_digest is None or result.get("manifest_sha256") != manifest_digest:
        return "run %s has no run-manifest.json matching its result" % claim["run_id"]
    if (
        result.get("status") != "complete"
        or result.get("child_exit") != 0
        or result.get("budget_failure")
    ):
        return "run %s did not complete" % claim["run_id"]
    if "test_overrides" in result:
        return "run %s used test overrides" % claim["run_id"]
    hard, telemetry = result.get("hard"), result.get("telemetry")
    actions = hard.get("scripted_actions") if isinstance(hard, dict) else None
    granted = actions.get("granted_ids") if isinstance(actions, dict) else None
    if not (
        isinstance(granted, list)
        and all(isinstance(g, str) for g in granted)
        and isinstance(telemetry, dict)
    ):
        return "run %s has a malformed result" % claim["run_id"]
    if actions.get("dispatched_without_permit"):
        return "run %s dispatched an action without a permit" % claim["run_id"]
    if claim["action_id"] not in granted:
        return "run %s never granted action %s" % (claim["run_id"], claim["action_id"])
    if not is_count(telemetry.get("events")):
        return "run %s recorded no telemetry events" % claim["run_id"]
    return None


def check_sdk(row, runs, report):
    rid, claim = row_id(row), row.get("sdk_report")
    if claim is None or claim == SDK_PENDING:
        report.gap(rid, "sdk_pending", "SDK report not yet executed (%s)" % SDK_PENDING)
        return
    problem = sdk_problem(claim, runs)
    if problem:
        report.lie(rid, "fabricated_sdk", problem)


def check_ledger(rows, baseline, tests, receipts, digest, sdk_runs):
    report = Report()
    valid = check_structure(rows, baseline, report)
    report.rows = len(rows)
    for row in valid:
        status = row["status"]
        report.by_status[status] = report.by_status.get(status, 0) + 1
        check_reason(row, report)
        if status == "uncovered":
            report.gap(
                row_id(row), "uncovered", row["reason"].strip() or "no reason recorded"
            )
            continue
        names = row_test_names(row, report)
        if status != "N/A" and not names:
            continue
        check_local(row, names, tests, receipts, digest, report)
        if status != "N/A":
            check_sdk(row, sdk_runs, report)
    return report


def load_receipts(path):
    if path is None:
        return []
    data = read_json(path, "receipts")
    if not (
        isinstance(data, dict)
        and data.get("schema") == RECEIPT_SCHEMA
        and isinstance(data.get("receipts"), list)
    ):
        raise Unusable(
            'receipts %s must be {"schema": %d, "receipts": [...]}'
            % (path, RECEIPT_SCHEMA)
        )
    return data["receipts"]


def command_check(args):
    rows = read_json(args.ledger, "ledger")
    baseline = load_baseline(args.synthetic_baseline)
    root = Path(args.source_root)
    if args.test_list:
        try:
            tests = parse_test_list(Path(args.test_list).read_text())
        except OSError as error:
            raise Unusable("cannot read test list %s: %s" % (args.test_list, error))
    else:
        tests = cargo_test_list(args.cargo, root)
    receipts = load_receipts(args.receipts)
    report = check_ledger(
        rows,
        baseline,
        tests,
        receipts,
        source_digest(root),
        load_sdk_runs(args.sdk_receipts),
    )
    strict = args.mode == "strict"
    failed = bool(report.integrity) or (strict and bool(report.gaps))
    if args.json:
        print(
            json.dumps(
                {
                    "mode": args.mode,
                    "baseline": "synthetic" if args.synthetic_baseline else "pinned",
                    "rows": report.rows,
                    "by_status": report.by_status,
                    "integrity": [list(i) for i in report.integrity],
                    "gaps": [list(g) for g in report.gaps],
                    "pass": not failed,
                },
                indent=1,
            )
        )
    else:
        if args.synthetic_baseline:
            print("note: checking against a synthetic baseline, not the pinned one")
        print_report(report, args.mode, failed)
    return 1 if failed else 0


def print_report(report, mode, failed):
    for rid, kind, detail in report.integrity:
        print("LEDGER  %-18s %s: %s" % (kind, rid, detail))
    kinds = {}
    for rid, kind, detail in report.gaps:
        kinds.setdefault(kind, []).append((rid, detail))
    for kind in sorted(kinds):
        print("GAP     %-18s %d row(s)" % (kind, len(kinds[kind])))
        for rid, detail in kinds[kind]:
            print("          %s: %s" % (rid, detail))
    counts = ", ".join("%s=%d" % item for item in sorted(report.by_status.items()))
    print(
        "recovery matrix (%s): %d rows (%s); %d ledger error(s), %d gap(s): %s"
        % (
            mode,
            report.rows,
            counts,
            len(report.integrity),
            len(report.gaps),
            "FAIL" if failed else "ok",
        )
    )


CARGO_RUNNING = re.compile(r"^running (\d+) tests?$", re.M)
CARGO_RESULT = re.compile(r"^test result: ok\. (\d+) passed;", re.M)


def run_receipt(cargo, root, full, digest):
    command = [cargo, "test", "--locked", "--lib", full, "--", "--exact"]
    try:
        done = subprocess.run(
            command,
            cwd=root,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            universal_newlines=True,
        )
    except OSError as error:
        raise Unusable("cannot run %s: %s" % (command[0], error))
    running = CARGO_RUNNING.search(done.stdout)
    result = CARGO_RESULT.search(done.stdout)
    return {
        "test": full,
        "command": command,
        "exit": done.returncode,
        "discovered": int(running.group(1)) if running else 0,
        "passed": int(result.group(1)) if result else 0,
        "source_digest": digest,
    }


def command_record(args):
    rows = read_json(args.ledger, "ledger")
    if not isinstance(rows, list):
        raise Unusable("ledger must be a JSON list of rows")
    root = Path(args.source_root)
    tests = (
        parse_test_list(Path(args.test_list).read_text())
        if args.test_list
        else cargo_test_list(args.cargo, root)
    )
    scratch = Report()
    wanted = []
    for row in rows:
        if isinstance(row, dict) and all(
            isinstance(row.get(f), str) for f in ("phase", "cause", "status", "test")
        ):
            if row["status"] != "uncovered":
                wanted.extend(row_test_names(row, scratch))
    names, skipped = [], []
    for name in dict.fromkeys(wanted):
        full, why = resolve_name(name, tests)
        (names if full else skipped).append(full or why)
    digest = source_digest(root)
    receipts, failed = [], 0
    for full in sorted(set(names)):
        receipt = run_receipt(args.cargo, root, full, digest)
        problem = receipt_problem(receipt, full, digest)
        print("%s %s" % ("ok  " if problem is None else "FAIL", full))
        if problem is None:
            receipts.append(receipt)
        else:
            failed += 1
            print("      %s" % problem[1])
    if source_digest(root) != digest:
        raise Unusable("sources changed while recording; receipts discarded")
    for why in skipped:
        print("skipped: %s" % why)
    # Only passing, nonempty runs become receipts; a failure leaves the test
    # without one, so a script that ignores the exit code cannot pass it.
    with open(args.out, "w") as handle:
        json.dump({"schema": RECEIPT_SCHEMA, "receipts": receipts}, handle, indent=1)
        handle.write("\n")
    print(
        "recorded %d receipt(s) to %s, %d failed, %d skipped"
        % (len(receipts), args.out, failed, len(skipped))
    )
    return 1 if failed or skipped else 0


def command_digest(args):
    print(source_digest(args.source_root))
    return 0


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p):
        p.add_argument(
            "--source-root", default=str(Path(__file__).resolve().parents[1])
        )
        p.add_argument("--cargo", default="cargo")
        p.add_argument(
            "--test-list",
            help="saved `cargo test --lib -- --list` output instead of running cargo",
        )

    check = sub.add_parser("check", help="validate a ledger")
    check.add_argument("ledger")
    check.add_argument(
        "--mode", choices=("incremental", "strict"), default="incremental"
    )
    check.add_argument("--receipts", help="local run receipts written by `record`")
    check.add_argument(
        "--sdk-receipts",
        help="directory holding run-budget artifacts (budget-result.json)",
    )
    check.add_argument(
        "--synthetic-baseline",
        help="JSON list of [phase, cause] replacing the pinned 174-row baseline",
    )
    check.add_argument("--json", action="store_true")
    common(check)
    check.set_defaults(run=command_check)

    record = sub.add_parser(
        "record", help="run the ledger's local tests and write receipts"
    )
    record.add_argument("ledger")
    record.add_argument("--out", required=True)
    common(record)
    record.set_defaults(run=command_record)

    digest = sub.add_parser("digest", help="print the source digest receipts bind to")
    digest.add_argument(
        "--source-root", default=str(Path(__file__).resolve().parents[1])
    )
    digest.set_defaults(run=command_digest)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return args.run(args)
    except Unusable as error:
        print("check-recovery-matrix: %s" % error, file=sys.stderr)
        return 2
    except (OSError, UnicodeError, RecursionError) as error:
        print("check-recovery-matrix: unusable input: %s" % error, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
