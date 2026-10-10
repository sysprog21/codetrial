"""Task sidecar acceptance and refusal at the instructor boundary."""

import ast
import copy
import json
import sys
import re
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from problem_bank.bank import validated_problems  # noqa: E402
from problem_bank.rules import posed, validated_variant  # noqa: E402
from problem_bank.tasks import (  # noqa: E402
    DEFAULT_CHECKS,
    DEFAULTS,
    OMITTED,
    validated_sidecar,
    validated_sidecars,
)


def fixture(name="customized"):
    return json.loads((ROOT / "tests/fixtures/task-mode" / f"{name}.json").read_text())


def validate(data):
    return validated_sidecar(
        data["problem"], data["judge"], data["variant"], data.get("sidecar", OMITTED)
    )


class TaskSidecarTests(unittest.TestCase):
    def test_customized_and_uncustomized_records_load(self):
        custom = validate(fixture())
        self.assertEqual(custom["completionTargets"][0]["id"], "closer-branch")
        default = validate(fixture("uncustomized"))
        self.assertEqual(default["understandingChecks"], DEFAULT_CHECKS)
        self.assertEqual(default["completionTargets"], [])
        self.assertEqual(default["maxHintRungs"], 3)

    def test_custom_starter_remains_partial_and_valid_after_variant_renames(self):
        data = fixture()
        shipped, graded = posed(data["problem"], data["judge"], data["variant"])
        code = shipped["starterCode"]["python"]
        self.assertEqual(code.count("TASK_COMPLETE_CLOSER"), 1)
        ast.parse(code)
        namespace = {}
        exec(code, namespace)
        run = getattr(namespace["Solution"](), graded["entry"])
        results = [run(*case["input"]) == case["expected"] for case in graded["cases"]]
        self.assertEqual(len(results), 7)
        self.assertTrue(any(results))
        self.assertFalse(all(results))

    def test_fixtures_pass_existing_bank_structure_checks(self):
        for name in ("customized", "uncustomized"):
            data = fixture(name)
            with tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "problems.json"
                path.write_text(json.dumps([data["problem"]]))
                self.assertEqual(len(validated_problems(path)), 1)
            validated_variant(data["problem"], data["judge"], data["variant"])
        data = fixture("uncustomized")
        bank = json.loads((ROOT / "problem-bank/problems.json").read_text())
        self.assertEqual(
            data["problem"], next(row for row in bank if row["id"] == "two-sum")
        )
        for name in ("judges", "variants"):
            source = json.loads((ROOT / "problem-bank" / f"{name}.json").read_text())
            self.assertEqual(
                data["judge" if name == "judges" else "variant"], source["two-sum"]
            )

    def test_distinct_targets_cannot_share_or_overlap_a_marker(self):
        for marker in ("TASK_COMPLETE_CLOSER", "TASK"):
            data = fixture()
            other = copy.deepcopy(data["sidecar"]["completionTargets"][0])
            other.update(id="another-branch", marker=marker)
            data["sidecar"]["completionTargets"].append(other)
            with (
                self.subTest(marker=marker),
                self.assertRaisesRegex(ValueError, "marker"),
            ):
                validate(data)

    def test_marker_is_checked_after_variant_renames(self):
        data = fixture()
        data["sidecar"]["completionTargets"][0]["marker"] = "TASK"
        data["problem"]["starterCode"]["python"] = "# TASK"
        data["variant"]["terms"] = {"TASK": "OTHER"}
        with self.assertRaisesRegex(ValueError, "marker"):
            validate(data)

    def test_literal_braces_beside_variables_are_valid(self):
        data = fixture()
        data["sidecar"]["interactionPrompt"] = "{ {{title}} {"
        self.assertEqual(validate(data)["interactionPrompt"], "{ {{title}} {")
        data["sidecar"]["interactionPrompt"] = "{" + "{{title}}" + "{"
        validate(data)

    def test_hint_ceiling_is_independent_of_ladder_length(self):
        data = fixture()
        data["variant"]["hints"] = ["one"] * 5
        data["sidecar"]["maxHintRungs"] = 4
        with self.assertRaisesRegex(ValueError, "maxHintRungs"):
            validate(data)

    def test_default_artifact_shape_and_duration_bounds(self):
        numeric = {
            "durationMin",
            "maxHintRungs",
            "extraChecks",
            "wrapUpSeconds",
            "wrapUpChecks",
            "nudgeIdleSeconds",
            "nudgeIntervalSeconds",
            "pinDigits",
            "taskBytes",
            "setBytes",
            "revisionSnapshots",
            "publicRunSummaries",
            "evidenceRevisions",
            "pendingAdmissionSeconds",
            "promptBytes",
            "fetchSeconds",
            "coreChecks",
            "runCaptureSeconds",
            "wrapUpAnswerSeconds",
            "reviewBytes",
            "reviewRetentionSeconds",
            "reviewDeliverySeconds",
            "lookAwaySeconds",
            "kdfIterations",
            "codeBytes",
        }
        self.assertEqual(
            set(DEFAULTS),
            numeric | {"language", "understandingChecks", "interactionPrompt"},
        )
        self.assertTrue(
            all(type(DEFAULTS[key]) is int and DEFAULTS[key] > 0 for key in numeric)
        )
        self.assertEqual(len(DEFAULT_CHECKS), DEFAULTS["coreChecks"])
        config = (ROOT / "src/config.rs").read_text()
        bounds = [
            int(re.search(rf"pub const {name}: u32 = ([0-9]+);", config).group(1))
            for name in ("MIN_DURATION_MIN", "MAX_DURATION_MIN")
        ]
        from problem_bank.task_package import DURATION_BOUNDS

        self.assertEqual(list(DURATION_BOUNDS), bounds)
        self.assertLessEqual(bounds[0], DEFAULTS["durationMin"])
        self.assertLessEqual(DEFAULTS["durationMin"], bounds[1])

    def test_rejects_unknown_fields_at_every_sidecar_level(self):
        for field in (None, "completionTargets", "understandingChecks"):
            data = fixture()
            target = data["sidecar"] if field is None else data["sidecar"][field][0]
            target["unknown"] = True
            with (
                self.subTest(field=field),
                self.assertRaisesRegex(ValueError, "exactly"),
            ):
                validate(data)

    def test_rejects_unknown_and_boolean_versions(self):
        for version in (0, 2, True, 1.0, "1", None):
            data = fixture()
            data["sidecar"]["promptVersion"] = version
            with (
                self.subTest(version=version),
                self.assertRaisesRegex(ValueError, "promptVersion"),
            ):
                validate(data)

    def test_rejects_missing_and_duplicate_markers(self):
        for replacement in ("", "TASK_COMPLETE_CLOSER TASK_COMPLETE_CLOSER"):
            data = fixture()
            data["problem"]["starterCode"]["python"] = replacement
            with (
                self.subTest(replacement=replacement),
                self.assertRaisesRegex(ValueError, "marker"),
            ):
                validate(data)

    def test_rejects_ambiguous_unresolved_and_duplicate_cases(self):
        for mode in ("ambiguous", "unresolved", "duplicate"):
            data = fixture()
            if mode == "ambiguous":
                data["judge"]["cases"].append(copy.deepcopy(data["judge"]["cases"][4]))
            elif mode == "unresolved":
                data["sidecar"]["requiredCases"] = ["nested"]
            else:
                data["sidecar"]["requiredCases"] *= 2
            with (
                self.subTest(mode=mode),
                self.assertRaisesRegex(ValueError, "required case"),
            ):
                validate(data)

    def test_rejects_unknown_and_malformed_variables(self):
        for prompt in (
            "Ask {{optimal}}",
            "Ask {{title",
            "Ask title}}",
            "Ask {{ title }}",
        ):
            data = fixture()
            data["sidecar"]["interactionPrompt"] = prompt
            with (
                self.subTest(prompt=prompt),
                self.assertRaisesRegex(ValueError, "variable"),
            ):
                validate(data)

    def test_rejects_oversize_utf8_prompt(self):
        data = fixture()
        # Byte width, rather than character count, sets the prompt budget.
        data["sidecar"]["interactionPrompt"] = "\u00e9" * 2049
        with self.assertRaisesRegex(ValueError, "4096 bytes"):
            validate(data)

    def test_rejects_hints_above_ladder(self):
        for limit in (4, -1, True, "3", 3.0):
            data = fixture()
            data["sidecar"]["maxHintRungs"] = limit
            with (
                self.subTest(limit=limit),
                self.assertRaisesRegex(ValueError, "maxHintRungs"),
            ):
                validate(data)
        data = fixture()
        data["variant"]["hints"] = ["one", "two"]
        with self.assertRaisesRegex(ValueError, "maxHintRungs"):
            validate(data)

    def test_omitted_sidecar_uses_shorter_ladder(self):
        data = fixture("uncustomized")
        data["variant"]["hints"] = ["one"]
        self.assertEqual(validate(data)["maxHintRungs"], 1)

    def test_refuses_duplicate_ids_invalid_languages_and_rubrics(self):
        for mode in ("target", "check", "language", "rubric", "marker"):
            data = fixture()
            sidecar = data["sidecar"]
            if mode == "target":
                sidecar["completionTargets"].append(
                    copy.deepcopy(sidecar["completionTargets"][0])
                )
            elif mode == "check":
                sidecar["understandingChecks"][1]["id"] = "trace"
            elif mode == "language":
                sidecar["completionTargets"][0]["language"] = "javascript"
            elif mode == "marker":
                sidecar["completionTargets"][0]["marker"] = "bad marker"
            else:
                sidecar["rubricProfile"] = "hiring"
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                validate(data)

    def test_rejects_missing_fields_and_wrong_types(self):
        for key in fixture()["sidecar"]:
            data = fixture()
            del data["sidecar"][key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate(data)
        for key in ("completionTargets", "understandingChecks", "requiredCases"):
            data = fixture()
            data["sidecar"][key] = {}
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate(data)

    def test_explicit_null_is_not_an_omitted_sidecar(self):
        data = fixture()
        data["sidecar"] = None
        with self.assertRaises(ValueError):
            validate(data)

    def test_bank_refuses_orphan_sidecars(self):
        data = fixture()
        with self.assertRaisesRegex(ValueError, "unknown task id"):
            validated_sidecars([data["problem"]], {}, {}, {"unknown": data["sidecar"]})

    def test_returned_sidecar_does_not_alias_input_or_defaults(self):
        data = fixture()
        validated = validate(data)
        validated["completionTargets"][0]["goal"] = "changed"
        self.assertNotEqual(data["sidecar"]["completionTargets"][0]["goal"], "changed")
        default = validate(fixture("uncustomized"))
        default["understandingChecks"][0]["question"] = "changed"
        self.assertNotEqual(DEFAULT_CHECKS[0]["question"], "changed")

    def test_judge_indexes_stay_within_what_codetrial_reads(self):
        from problem_bank.task_structure import judge_options

        judge_options({"kind": "function", "outputParam": 2**64 - 1})
        for value in (-1, 2**64, True):
            with self.subTest(value=value), self.assertRaises(ValueError):
                judge_options({"kind": "function", "outputParam": value})


if __name__ == "__main__":
    unittest.main()
