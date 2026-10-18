"""Authenticated packaging, safe projections and instructor secret handling."""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from problem_bank import task_package  # noqa: E402
from problem_bank.task_package import (  # noqa: E402
    build_package,
    checked_manifest,
    checked_pin,
    decrypt,
    encrypt,
    load_bank,
    preview,
    publish_scan,
    render_prompt,
    rules_in_words,
    validate_package,
    validated_rules,
)
from problem_bank.tasks import DEFAULTS  # noqa: E402
from cryptography.exceptions import InvalidTag  # noqa: E402

PIN = "583209"
SITE = "https://teacher.github.io/course"


def package():
    fixtures = [
        json.loads((ROOT / "tests/fixtures/task-mode" / f"{name}.json").read_text())
        for name in ("customized", "uncustomized")
    ]
    return {
        "packageVersion": 1,
        "setId": "classroom",
        "setVersion": 7,
        "rules": validated_rules(None),
        "problems": [row["problem"] for row in fixtures],
        "judges": {row["problem"]["id"]: row["judge"] for row in fixtures},
        "variants": {row["problem"]["id"]: row["variant"] for row in fixtures},
        "sidecars": {
            row["problem"]["id"]: row["sidecar"] for row in fixtures if "sidecar" in row
        },
    }


def bank_files(directory, value):
    for source, target in (
        ("problems", "problems"),
        ("judges", "judges"),
        ("variants", "variants"),
        ("sidecars", "task-mode"),
    ):
        (directory / f"{target}.json").write_text(json.dumps(value[source]))


def fixture_package():
    """The checked-in package and its manifest, for tests that only read what
    is published and never derive its key."""
    fixtures = ROOT / "tests/fixtures/task-mode"
    return (
        (fixtures / "package.enc").read_bytes(),
        json.loads((fixtures / "manifest.json").read_text()),
    )


def published(directory, ciphertext, manifest):
    path = directory / "sets/classroom/7"
    path.mkdir(parents=True)
    (path / "tasks.enc").write_bytes(ciphertext)
    (path / "manifest.json").write_text(json.dumps(manifest))
    return path


class TaskPackageTests(unittest.TestCase):
    def test_instructor_preview_matches_readable_runtime_golden(self):
        for task_id in ("delimiter-closer", "two-sum"):
            expected = (
                ROOT / "tests/golden/task-preview" / f"{task_id}.txt"
            ).read_text()
            self.assertEqual(render_prompt(package(), task_id), expected)

    def test_the_prompt_shows_jim_the_starter_the_attempt_opens_on(self):
        value = package()
        value["sidecars"]["delimiter-closer"]["languages"] = ["python", "java"]
        for language in ("python", "java"):
            prompt = render_prompt(value, "delimiter-closer", language)
            code = prompt.split("BEGIN UNTRUSTED LEARNER CODE\n")[1]
            code = code.split("\nEND UNTRUSTED LEARNER CODE")[0]
            starter = preview(value, "delimiter-closer")["starterCode"][language]
            self.assertEqual(json.loads(code), starter)
            # Renamed as the variant poses it, never the published name.
            self.assertIn("delimitersNestCleanly", starter)

    def test_a_task_names_its_languages_and_each_attempt_sees_its_own(self):
        value = package()
        problem = value["problems"][0]
        problem["starterCode"]["javascript"] = (
            "function isValid(s) {\n  // TASK_COMPLETE_CLOSER\n}\n"
        )
        sidecar = value["sidecars"]["delimiter-closer"]
        sidecar["languages"] = ["javascript", "python"]
        sidecar["completionTargets"].append(
            dict(
                sidecar["completionTargets"][0],
                id="closer-js",
                language="javascript",
            )
        )
        validate_package(value)
        # The first language is the one the preview shows by default.
        first = render_prompt(value, "delimiter-closer")
        self.assertIn("language=javascript;", first)
        self.assertIn('"closer-js"', first)
        self.assertNotIn('"closer-branch"', first)
        python = render_prompt(value, "delimiter-closer", "python")
        self.assertIn("language=python;", python)
        self.assertNotIn('"closer-js"', python)
        with self.assertRaises(ValueError):
            render_prompt(value, "delimiter-closer", "java")

    def test_cli_build_reads_the_pin_without_echo_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            (bank / "task-set.json").write_text(json.dumps({"lookAwaySeconds": 12}))
            command = [sys.executable, str(ROOT / "scripts/task-package.py")]
            rendered = subprocess.run(
                command
                + ["render-prompt", "--bank", str(bank)]
                + ["--task-id", "delimiter-closer"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(rendered.returncode, 0, rendered.stderr)
            args = command + [
                "build",
                "--bank",
                str(bank),
                "--set-id",
                "classroom",
                "--version",
                "7",
                "--site",
                SITE + "/",
                "--output",
                str(directory / "publish"),
            ]
            built = subprocess.run(
                args, input=PIN + "\n", capture_output=True, text=True
            )
            self.assertEqual(built.returncode, 0, built.stderr)
            self.assertNotIn(PIN, built.stdout + built.stderr)
            publish = directory / "publish"
            self.assertEqual(publish_scan(publish), 1)
            version = publish / "sets/classroom/7"
            manifest = json.loads((version / "manifest.json").read_text())
            source = decrypt((version / "tasks.enc").read_bytes(), manifest, PIN)
            self.assertEqual(source["rules"]["lookAwaySeconds"], 12)
            for page in (publish / "index.html", version / "index.html"):
                self.assertNotIn(PIN, page.read_text())
            before = {
                path: path.read_bytes() for path in publish.rglob("*") if path.is_file()
            }
            # No PIN is given: a build that asked for one would fail on reading
            # it, after the key derivation the refusal should spare.
            refused = subprocess.run(args, input="", capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("already holds version 7", refused.stderr)
            self.assertNotIn("Building", refused.stdout)
            after = {
                path: path.read_bytes() for path in publish.rglob("*") if path.is_file()
            }
            self.assertEqual(after, before)

    def test_a_build_into_an_output_the_scan_refuses_writes_nothing(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            output = directory / "publish"
            build_package(load_bank(bank, "classroom", 7), PIN, SITE, output)
            (output / "stray.txt").write_text("private data")
            listing = (output / "index.html").read_text()
            with self.assertRaisesRegex(ValueError, "stray.txt"):
                build_package(load_bank(bank, "classroom", 8), PIN, SITE, output)
            self.assertFalse((output / "sets/classroom/8").exists())
            self.assertEqual((output / "index.html").read_text(), listing)

    def test_a_build_that_fails_after_writing_leaves_the_output_as_it_was(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            output = directory / "publish"
            build_package(load_bank(bank, "classroom", 7), PIN, SITE, output)
            before = {
                path: path.read_bytes() for path in output.rglob("*") if path.is_file()
            }
            fresh = directory / "fresh"
            # The scan fails on this thread alone: scripts/run-python-tests.py
            # runs the other cases beside it in this process.
            this_thread, real_scan = threading.get_ident(), task_package.publish_scan

            def refused_here(directory):
                if threading.get_ident() == this_thread:
                    raise ValueError("refused")
                return real_scan(directory)

            scan = mock.patch.object(task_package, "publish_scan", refused_here)
            for place, version in ((output, 8), (fresh, 1)):
                with self.subTest(place=place.name), scan:
                    with self.assertRaisesRegex(ValueError, "refused"):
                        build_package(
                            load_bank(bank, "classroom", version), PIN, SITE, place
                        )
            after = {
                path: path.read_bytes() for path in output.rglob("*") if path.is_file()
            }
            self.assertEqual(after, before)
            self.assertEqual(
                sorted(path.name for path in (output / "sets/classroom").iterdir()),
                ["7"],
            )
            self.assertFalse(fresh.exists())

    def test_version_pages_announce_rules_and_link_each_task_at_its_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            output = directory / "publish"
            build_package(load_bank(bank, "classroom", 7), PIN, SITE, output)
            build_package(load_bank(bank, "classroom", 8), PIN, SITE, output)
            self.assertEqual(publish_scan(output), 2)
            page = (output / "sets/classroom/8/index.html").read_text()
            self.assertIn(
                "http://localhost:3000/t/classroom/delimiter-closer?"
                "site=https%3A%2F%2Fteacher.github.io%2Fcourse&amp;version=8",
                page,
            )
            for words in ("fullscreen", "invalid", "8 seconds", "calculator"):
                self.assertIn(words, page)
            listing = (output / "index.html").read_text()
            self.assertIn('href="sets/classroom/7/"', listing)
            self.assertIn('href="sets/classroom/8/"', listing)
            self.assertTrue((output / ".nojekyll").is_file())

    def test_all_bank_prompts_exclude_private_notes_and_guide_text(self):
        problems = json.loads((ROOT / "problem-bank/problems.json").read_text())
        judges = json.loads((ROOT / "problem-bank/judges.json").read_text())
        variants = json.loads((ROOT / "problem-bank/variants.json").read_text())
        guides = json.loads((ROOT / "problem-bank/guides.json").read_text())["notes"]
        for problem in problems:
            task_id = problem["id"]
            source = {
                "packageVersion": 1,
                "setId": "classroom",
                "setVersion": 7,
                "rules": validated_rules(None),
                "problems": [problem],
                "judges": {task_id: judges[task_id]},
                "variants": {task_id: variants[task_id]},
                "sidecars": {},
            }
            prompt = render_prompt(source, task_id)
            self.assertNotIn(problem["optimal"], prompt)
            self.assertNotIn(problem["pitfalls"], prompt)
            if task_id in guides:
                self.assertNotIn(guides[task_id], prompt)

    def test_task_ids_and_malformed_template_refusals_match_server(self):
        for task_id in ("Two_Sum", "-problem", "x" * 65, "../escape"):
            source = package()
            source["problems"][0]["id"] = task_id
            with self.assertRaises(ValueError):
                validate_package(source)
        source = package()
        source["sidecars"]["delimiter-closer"]["interactionPrompt"] = "{{{{title}}"
        with self.assertRaises(ValueError):
            validate_package(source)

    def test_shared_downloaded_record_refusals(self):
        refusals = json.loads(
            (ROOT / "tests/fixtures/task-mode/refusals.json").read_text()
        )
        for refusal in refusals:
            row = json.loads(
                (
                    ROOT
                    / "tests/fixtures/task-mode"
                    / (refusal.get("fixture", "customized") + ".json")
                ).read_text()
            )
            for patch in refusal.get("patches", [refusal]):
                target = row
                for key in patch["path"][:-1]:
                    target = (
                        target[int(key)] if isinstance(target, list) else target[key]
                    )
                key = patch["path"][-1]
                if patch.get("remove"):
                    del target[key]
                else:
                    target[int(key) if isinstance(target, list) else key] = patch[
                        "value"
                    ]
            if refusal.get("structural"):
                from problem_bank.task_structure import structural

                with (
                    self.subTest(refusal=refusal["name"]),
                    self.assertRaises(ValueError),
                ):
                    structural(row["problem"], row["judge"], row["variant"])
            source = {
                "packageVersion": 1,
                "setId": "classroom",
                "setVersion": 7,
                "rules": validated_rules(None),
                "problems": [row["problem"]],
                "judges": {row["problem"]["id"]: row["judge"]},
                "variants": {row["problem"]["id"]: row["variant"]},
                "sidecars": {row["problem"]["id"]: row["sidecar"]}
                if "sidecar" in row
                else {},
            }
            with self.subTest(refusal=refusal["name"]), self.assertRaises(ValueError):
                validate_package(source)

    def test_posed_bank_golden_matches_python_judges_starters_and_notes(self):
        from problem_bank.rules import posed
        from problem_bank.task_package import compact

        problems = json.loads((ROOT / "problem-bank/problems.json").read_text())
        judges = json.loads((ROOT / "problem-bank/judges.json").read_text())
        variants = json.loads((ROOT / "problem-bank/variants.json").read_text())
        expected = json.loads((ROOT / "tests/golden/task-posed.json").read_text())
        for problem in problems:
            task_id = problem["id"]
            source, judge = posed(problem, judges[task_id], variants[task_id])
            value = {
                "judge": judge,
                "starterCode": source["starterCode"],
                "optimal": source["optimal"],
                "pitfalls": source["pitfalls"],
            }
            self.assertEqual(
                hashlib.sha256(compact(value)).hexdigest(), expected[task_id], task_id
            )

    def test_renamed_class_method_matches_runtime_practice_golden(self):
        row = json.loads(
            (ROOT / "tests/fixtures/task-mode/class-renamed-method.json").read_text()
        )
        source = {
            "packageVersion": 1,
            "setId": "classroom",
            "setVersion": 7,
            "rules": validated_rules(None),
            "problems": [row["problem"]],
            "judges": {"min-stack": row["judge"]},
            "variants": {"min-stack": row["variant"]},
            "sidecars": {},
        }
        actual = preview(source, "min-stack")
        expected = json.loads(
            (ROOT / "tests/golden/task-preview/class-renamed-method.json").read_text()
        )
        self.assertEqual(actual["publicCases"], expected["judge"])
        # The preview shows the one language a task without a sidecar offers.
        self.assertEqual(
            actual["starterCode"], {"python": expected["starterCode"]["python"]}
        )

    def test_encryption_roundtrip_uses_fresh_nonces_and_salts(self):
        source = package()
        one, first = encrypt(source, PIN)
        two, second = encrypt(source, PIN)
        self.assertEqual(decrypt(one, first, PIN), source)
        self.assertEqual(decrypt(two, second, PIN), source)
        self.assertNotEqual(one[:12], two[:12])
        self.assertNotEqual(first["salt"], second["salt"])

    def test_the_package_the_server_tests_open_is_what_this_side_builds(self):
        # The Rust suite opens this file with PIN 123456; decrypting it here
        # holds the key derivation and the associated data on both sides to
        # the same bytes. After a fixture change, rebuild it with
        # `encrypt(package(), "123456")`.
        ciphertext, manifest = fixture_package()
        self.assertEqual(decrypt(ciphertext, manifest, "123456"), package())
        with self.assertRaises(InvalidTag):
            decrypt(ciphertext, manifest, PIN)

    def test_wrong_pin_and_authenticated_tag_tampering_fail(self):
        data, manifest = encrypt(package(), PIN)
        with self.assertRaises(InvalidTag):
            decrypt(data, manifest, "583210")
        tampered = data[:-1] + bytes([data[-1] ^ 1])
        manifest["ciphertextSha256"] = hashlib.sha256(tampered).hexdigest()
        with self.assertRaises(InvalidTag):
            decrypt(tampered, manifest, PIN)

    def test_manifest_metadata_and_salt_are_bound_to_the_key(self):
        data, manifest = encrypt(package(), PIN)
        for field, value in (("setVersion", 8), ("salt", "AAAAAAAAAAAAAAAAAAAAAA==")):
            with self.subTest(field=field), self.assertRaises(InvalidTag):
                decrypt(data, dict(manifest, **{field: value}), PIN)

    def test_bad_schema_and_oversize_packages_are_refused(self):
        for mutation in ("field", "version", "sidecar", "task-id", "rules"):
            value = package()
            if mutation == "field":
                value["unknown"] = True
            elif mutation == "version":
                value["packageVersion"] = True
            elif mutation == "sidecar":
                value["sidecars"]["delimiter-closer"]["maxHintRungs"] = 4
            elif mutation == "rules":
                value["rules"] = {"durationMin": 15}
            else:
                value["sidecars"]["missing-task"] = value["sidecars"][
                    "delimiter-closer"
                ]
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                validate_package(value)
        value = package()
        value["problems"][0]["optimal"] = "x" * (DEFAULTS["taskBytes"] + 1)
        with self.assertRaisesRegex(ValueError, "task exceeds|32 KiB"):
            validate_package(value)
        value["problems"][0]["optimal"] = "x" * (DEFAULTS["setBytes"] + 1)
        with self.assertRaisesRegex(ValueError, "set exceeds"):
            validate_package(value)
        # A starter the runtime could not capture as a revision.
        value = package()
        row = next(r for r in value["problems"] if r["id"] == "delimiter-closer")
        row["starterCode"]["python"] += "#" * 32001 + "\n"
        with self.assertRaisesRegex(ValueError, "code byte limit"):
            validate_package(value)

    def test_set_rules_take_defaults_and_refuse_out_of_range_values(self):
        rules = validated_rules({"durationMin": 20, "closesAt": "2026-12-31T23:59:59Z"})
        self.assertEqual(rules["durationMin"], 20)
        self.assertEqual(rules["lookAwaySeconds"], DEFAULTS["lookAwaySeconds"])
        for bad in (
            {"unknown": 1},
            {"durationMin": 5},
            {"durationMin": True},
            {"maxHintRungs": DEFAULTS["maxHintRungs"] + 1},
            {"lookAwaySeconds": 4},
            {"closesAt": "2026-12-31"},
            # The right shape, but no such time: the server refuses to parse it.
            {"closesAt": "2026-02-31T00:00:00Z"},
            {"closesAt": "2026-12-31T24:00:00Z"},
            {"closesAt": "1969-12-31T23:59:59Z"},
            # Arabic-Indic digits, which `\d` alone would take.
            {"closesAt": "\u0662026-12-31T23:59:59Z"},
            {"rulesNote": " "},
            {"rulesNote": "x" * 1025},
        ):
            with self.subTest(rules=bad), self.assertRaises(ValueError):
                validated_rules(bad)

    def test_uncustomized_sidecar_defaults_and_live_prompt_privacy(self):
        value = package()
        entries, sidecars = validate_package(value)
        self.assertEqual(
            sidecars["two-sum"]["understandingChecks"], DEFAULTS["understandingChecks"]
        )
        self.assertEqual(len(entries), 2)
        for row in value["problems"]:
            prompt = render_prompt(value, row["id"])
            self.assertNotIn(row["optimal"], prompt)
            self.assertNotIn(row["pitfalls"], prompt)
            self.assertNotIn('"optimal"', prompt)
            self.assertNotIn('"pitfalls"', prompt)
            self.assertNotIn('"guide"', prompt)
        # The set's ceiling lowers what the instructor's preview and prompt
        # announce, as the server lowers what the learner gets.
        capped = dict(value, rules=validated_rules({"maxHintRungs": 0}))
        self.assertIn('"hintRungsMax":0', render_prompt(capped, "delimiter-closer"))
        self.assertEqual(
            preview(capped, "delimiter-closer")["rules"]["maxHintRungs"], 0
        )
        public = preview(value, "delimiter-closer")
        self.assertIn("TASK_COMPLETE_CLOSER", public["starterCode"]["python"])

    def test_preview_is_what_the_learner_page_is_sent(self):
        value = package()
        value["sidecars"]["delimiter-closer"]["languages"] = ["javascript", "python"]
        public = preview(value, "delimiter-closer")
        # The fields `load_task` in src/tasks/access.rs sends the page.
        self.assertEqual(
            set(public),
            {
                "title",
                "brief",
                "contract",
                "languages",
                "starterCode",
                "publicCases",
                "requiredCases",
                "completionTargets",
                "understandingChecks",
                "rules",
            },
        )
        variant = value["variants"]["delimiter-closer"]
        self.assertEqual(public["contract"], variant["contract"])
        self.assertEqual(public["languages"], ["javascript", "python"])
        self.assertEqual(set(public["starterCode"]), {"javascript", "python"})
        self.assertEqual(public["rules"], value["rules"])
        # Bound by source label, as indexes into the cases the page shows.
        cases = public["publicCases"]["cases"]
        self.assertEqual(public["requiredCases"], [4, 2, 5])
        self.assertEqual(public["publicCases"]["entry"], variant["entry"])
        self.assertEqual(len(cases), len(value["judges"]["delimiter-closer"]["cases"]))
        # An imported problem's published title stays off the page.
        two_sum = preview(value, "two-sum")
        title = value["problems"][1]["title"]
        self.assertNotIn("source", two_sum)
        self.assertNotIn(title, json.dumps(two_sum))

    def test_publish_scan_rejects_plaintext_banks_and_symlinks(self):
        data, manifest = fixture_package()
        for filename in ("problems.json", "secret.key", "task-set.json"):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                path = published(directory, data, manifest)
                self.assertEqual(publish_scan(directory), 1)
                for place in (directory, path):
                    (place / filename).write_text("private data")
                    with self.subTest(filename=filename, place=str(place)):
                        with self.assertRaises(ValueError):
                            publish_scan(directory)
                    (place / filename).unlink()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = published(directory, data, manifest)
            for allowed in ("CNAME", "README.md", ".nojekyll"):
                (directory / allowed).write_text("public")
            self.assertEqual(publish_scan(directory), 1)
            (directory / "leak").symlink_to(path / "tasks.enc")
            with self.assertRaises(ValueError):
                publish_scan(directory)

    def test_publish_scan_refuses_what_the_layout_does_not_name(self):
        data, manifest = fixture_package()
        for name, damage in (
            ("orphan landing page", "sets/classroom/8/index.html"),
            ("orphan ciphertext", "sets/classroom/8/tasks.enc"),
            ("empty version", "sets/classroom/8/"),
            ("empty set", "sets/other/"),
            ("directory at the root", "assets/"),
            ("empty directory at the root", "empty/"),
        ):
            with tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                published(directory, data, manifest)
                path = directory / damage
                if damage.endswith("/"):
                    path.mkdir(parents=True)
                    if name == "directory at the root":
                        (path / "logo.png").write_bytes(b"png")
                else:
                    path.parent.mkdir(parents=True)
                    path.write_bytes(data)
                with self.subTest(name), self.assertRaises(ValueError):
                    publish_scan(directory)

    def test_manifests_refuse_a_malformed_hash_or_size(self):
        data, manifest = fixture_package()
        for field, value in (
            ("ciphertextSha256", "\u00e9"),
            ("ciphertextSha256", 123),
            ("ciphertextSha256", manifest["ciphertextSha256"].upper()),
            ("ciphertextSha256", manifest["ciphertextSha256"][:-1]),
            ("ciphertextBytes", 0),
            ("ciphertextBytes", DEFAULTS["setBytes"] + 29),
        ):
            broken = dict(manifest, **{field: value})
            with self.subTest(field=field, value=value):
                with self.assertRaisesRegex(ValueError, "manifest"):
                    checked_manifest(broken)
                with self.assertRaisesRegex(ValueError, "manifest"):
                    decrypt(data, broken, "123456")
                with tempfile.TemporaryDirectory() as temporary:
                    published(Path(temporary), data, broken)
                    with self.assertRaisesRegex(ValueError, "manifest"):
                        publish_scan(Path(temporary))

    def test_publish_scan_rejects_corrupt_ciphertext(self):
        data, manifest = fixture_package()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            path = published(directory, data, manifest)
            (path / "tasks.enc").write_bytes(data + b"tampering")
            with self.assertRaises(ValueError):
                publish_scan(directory)

    def test_build_refuses_a_site_that_is_not_plain_https(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            for site in (
                "http://teacher.github.io/course",
                "https://user:secret@teacher.github.io",
                "https://teacher.github.io/course?x=1",
                # Empty, but still a query or fragment to CodeTrial.
                "https://teacher.github.io/course?",
                "https://teacher.github.io/course#",
                "https://teacher.github.io/course/?#",
                # A port `urlsplit` only checks when asked for it.
                "https://teacher.github.io:99999/course",
                "https://teacher.github.io:port/course",
            ):
                with self.subTest(site=site), self.assertRaises(ValueError):
                    build_package(
                        load_bank(bank, "classroom", 7),
                        PIN,
                        site,
                        directory / "publish",
                    )

    def test_problems_refuse_malformed_constraints_and_published_examples(self):
        # Read only here, to keep the source out of what the learner reads:
        # refused as the bank's mistake, never as a crash in that check.
        for name, damage in (
            ("no constraints", lambda row: row.pop("constraints")),
            ("constraints as text", lambda row: row.update(constraints="1 <= n")),
            ("oversize constraint", lambda row: row.update(constraints=["x" * 32769])),
            ("too many constraints", lambda row: row.update(constraints=["n"] * 17)),
            ("blank constraint", lambda row: row.update(constraints=[" "])),
            ("example as a number", lambda row: row.update(examples=[1])),
            ("example without input", lambda row: row["examples"][0].pop("input")),
            ("example input as data", lambda row: row["examples"][0].update(input=[2])),
            ("example extra field", lambda row: row["examples"][0].update(note="x")),
            ("too many examples", lambda row: row.update(examples=row["examples"] * 9)),
        ):
            value = package()
            damage(value["problems"][1])
            with self.subTest(name), self.assertRaises(ValueError):
                validate_package(value)

    def test_custom_bank_has_no_catalog_membership_requirement(self):
        value = package()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank_files(directory, value)
            self.assertEqual(load_bank(directory, "classroom", 7), value)

    def test_landing_page_rules_read_exactly_as_the_workspace_states_them(self):
        node = shutil.which("node")
        if node is None:
            self.skipTest("node is needed to run the workspace's wording")
        script = (
            "import {rulesInWords} from './web/task-controller.js';"
            "const cases = JSON.parse(process.argv[1]);"
            "console.log(JSON.stringify(cases.map(rulesInWords)));"
        )
        cases = [
            validated_rules(None),
            validated_rules(
                {
                    "durationMin": 20,
                    "lookAwaySeconds": 9,
                    "closesAt": "2026-12-31T23:59:59Z",
                    "rulesNote": "Bring paper.",
                }
            ),
        ]
        page = subprocess.run(
            [node, "--input-type=module", "-e", script, json.dumps(cases)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertEqual(
            json.loads(page.stdout), [rules_in_words(rules) for rules in cases]
        )

    def test_pin_rejects_non_ascii_and_non_six_digit_values(self):
        for pin in ("12345", "1234567", "abcdef", "\u0661" * 6):
            with self.subTest(pin=pin), self.assertRaises(ValueError):
                checked_pin(pin)


if __name__ == "__main__":
    unittest.main()
