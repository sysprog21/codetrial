"""Authenticated packaging, safe projections and instructor secret handling."""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from problem_bank.task_package import (  # noqa: E402
    build,
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
            refused = subprocess.run(
                args, input=PIN + "\n", capture_output=True, text=True
            )
            self.assertNotEqual(refused.returncode, 0)

    def test_version_pages_announce_rules_and_link_each_task_at_its_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "bank"
            bank.mkdir()
            bank_files(bank, package())
            output = directory / "publish"
            build(bank, "classroom", 7, PIN, SITE, output)
            build(bank, "classroom", 8, PIN, SITE, output)
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
                target[int(key) if isinstance(target, list) else key] = patch["value"]
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
        self.assertEqual(
            {"judge": actual["judge"], "starterCode": actual["problem"]["starterCode"]},
            expected,
        )

    def test_encryption_roundtrip_uses_fresh_nonces_and_salts(self):
        source = package()
        one, first = encrypt(source, PIN)
        two, second = encrypt(source, PIN)
        self.assertEqual(decrypt(one, first, PIN), source)
        self.assertEqual(decrypt(two, second, PIN), source)
        self.assertNotEqual(one[:12], two[:12])
        self.assertNotEqual(first["salt"], second["salt"])

    def test_wrong_pin_and_authenticated_tag_tampering_fail(self):
        data, manifest = encrypt(package(), PIN)
        with self.assertRaises(InvalidTag):
            decrypt(data, manifest, "583210")
        tampered = data[:-1] + bytes([data[-1] ^ 1])
        manifest["ciphertextSha256"] = hashlib.sha256(tampered).hexdigest()
        with self.assertRaises(InvalidTag):
            decrypt(tampered, manifest, PIN)

    def test_manifest_metadata_and_salt_are_bound_to_the_key(self):
        for field, value in (("setVersion", 8), ("salt", "AAAAAAAAAAAAAAAAAAAAAA==")):
            data, manifest = encrypt(package(), PIN)
            manifest[field] = value
            with self.subTest(field=field), self.assertRaises(InvalidTag):
                decrypt(data, manifest, PIN)

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
        self.assertIn(
            "TASK_COMPLETE_CLOSER", public["problem"]["starterCode"]["python"]
        )
        self.assertEqual(public["resultsLabel"], "Learner-reported practice evidence")

    def test_publish_scan_rejects_plaintext_banks_and_symlinks(self):
        data, manifest = encrypt(package(), PIN)
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

    def test_publish_scan_rejects_corrupt_ciphertext(self):
        data, manifest = encrypt(package(), PIN)
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
                # A port `urlsplit` only checks when asked for it.
                "https://teacher.github.io:99999/course",
                "https://teacher.github.io:port/course",
            ):
                with self.subTest(site=site), self.assertRaises(ValueError):
                    build(bank, "classroom", 7, PIN, site, directory / "publish")

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
