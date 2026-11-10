import copy
import importlib.util
import itertools
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "calibrate_framework", ROOT / "scripts" / "calibrate-framework.py"
)
CAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAL)


class CalibrationTests(unittest.TestCase):
    def fixture(self, name):
        return json.loads(
            (ROOT / "tests" / "fixtures" / "calibration" / name).read_text()
        )

    def all_scored_fixture(self):
        raw = self.fixture("passing-synthetic.json")
        for sample in raw["samples"]:
            rows = [sample["modelScores"]] + [
                rating["scores"] for rating in sample["humanRatings"]
            ]
            for row in rows:
                for phase in CAL.PHASES:
                    if row[phase] is None:
                        row[phase] = 82
        return raw

    def test_unassessable_coverage_zero_and_one(self):
        raw = self.all_scored_fixture()
        uncovered = CAL.analyze(raw)
        self.assertEqual(uncovered["status"], "NOT_CALIBRATED")
        self.assertEqual(uncovered["failures"], ["unassessable phase coverage missing"])
        for sample_index, phase in itertools.product(
            range(len(raw["samples"])), CAL.PHASES
        ):
            with self.subTest(sample=sample_index, phase=phase):
                one = copy.deepcopy(raw)
                sample = one["samples"][sample_index]
                sample["modelScores"][phase] = None
                for rating in sample["humanRatings"]:
                    rating["scores"][phase] = None
                covered = CAL.analyze(one)
                self.assertEqual(covered["status"], "PASS", covered["failures"])
                self.assertEqual(covered["counts"]["assessedPhasePairs"], 299)
                for metric in ("interRaterQwk", "modelHumanMae", "bandAgreement"):
                    self.assertEqual(
                        covered["metrics"][metric], uncovered["metrics"][metric]
                    )

    def test_all_unassessable_data_fails_without_numeric_metrics(self):
        raw = self.fixture("passing-synthetic.json")
        for sample in raw["samples"]:
            sample["modelScores"] = dict.fromkeys(CAL.PHASES)
            for rating in sample["humanRatings"]:
                rating["scores"] = dict.fromkeys(CAL.PHASES)
        result = CAL.analyze(raw)
        self.assertEqual(result["status"], "NOT_CALIBRATED")
        self.assertEqual(result["counts"]["sessions"], len(raw["samples"]))
        self.assertEqual(result["counts"]["assessedPhasePairs"], 0)
        self.assertNotIn("unassessable phase coverage missing", result["failures"])
        self.assertTrue(any("phase coverage missing" in f for f in result["failures"]))
        for metric in ("interRaterQwk", "modelHumanMae", "bandAgreement"):
            self.assertIsNone(result["metrics"][metric])
        for subgroup in result["metrics"]["subgroups"].values():
            self.assertEqual(subgroup["eligibleCohorts"], {})
            self.assertEqual(subgroup["insufficientCohorts"], {})
            self.assertIsNone(subgroup["maxMaeGap"])

    def test_assessability_mismatch_rejected_in_every_reviewer_order(self):
        for model_score in (30, None):
            raw = self.fixture("passing-synthetic.json")
            sample = raw["samples"][0]
            sample["modelScores"]["Repeat"] = model_score
            third = copy.deepcopy(sample["humanRatings"][0])
            third["reviewerId"] = "reviewer-c"
            third["scores"]["Repeat"] = None
            for ratings in itertools.permutations([*sample["humanRatings"], third]):
                with self.subTest(
                    model=model_score, order=[r["reviewerId"] for r in ratings]
                ):
                    sample["humanRatings"] = list(ratings)
                    with self.assertRaisesRegex(ValueError, "assessability mismatch"):
                        CAL.analyze(raw)

    def test_invalid_unassessable_samples_are_rejected(self):
        for defect in (
            "missing reviewer",
            "duplicate reviewer",
            "model key",
            "human key",
        ):
            with self.subTest(defect=defect):
                raw = self.fixture("passing-synthetic.json")
                sample = raw["samples"][0]
                sample["modelScores"] = dict.fromkeys(CAL.PHASES)
                for rating in sample["humanRatings"]:
                    rating["scores"] = dict.fromkeys(CAL.PHASES)
                if defect == "missing reviewer":
                    sample["humanRatings"].pop()
                elif defect == "duplicate reviewer":
                    sample["humanRatings"][1]["reviewerId"] = "reviewer-a"
                elif defect == "model key":
                    del sample["modelScores"]["Repeat"]
                else:
                    del sample["humanRatings"][0]["scores"]["Repeat"]
                with self.assertRaises(ValueError):
                    CAL.analyze(raw)

    def test_cli_assessability_statuses_and_exit_codes(self):
        zero = self.all_scored_fixture()
        one = copy.deepcopy(zero)
        sample = one["samples"][0]
        sample["modelScores"]["Repeat"] = None
        for rating in sample["humanRatings"]:
            rating["scores"]["Repeat"] = None
        mixed = copy.deepcopy(zero)
        third = copy.deepcopy(mixed["samples"][0]["humanRatings"][0])
        third["reviewerId"] = "reviewer-c"
        third["scores"]["Repeat"] = None
        mixed["samples"][0]["humanRatings"].append(third)
        cases = (
            ("control", self.fixture("passing-synthetic.json"), 0, "PASS"),
            ("zero", zero, 1, "NOT_CALIBRATED"),
            ("one", one, 0, "PASS"),
            ("mixed", mixed, 2, None),
        )
        with tempfile.TemporaryDirectory() as directory:
            for name, raw, code, status in cases:
                input_path = Path(directory) / f"{name}.json"
                input_path.write_text(json.dumps(raw), encoding="utf-8")
                for output_format in ("json", "markdown"):
                    with self.subTest(case=name, format=output_format):
                        result = subprocess.run(
                            [
                                sys.executable,
                                str(ROOT / "scripts/calibrate-framework.py"),
                                str(input_path),
                                "--format",
                                output_format,
                            ],
                            capture_output=True,
                            text=True,
                            check=False,
                        )
                        self.assertEqual(result.returncode, code, result.stderr)
                        if code == 2:
                            self.assertEqual(result.stdout, "")
                            self.assertIn("assessability mismatch", result.stderr)
                        else:
                            self.assertEqual(result.stderr, "")
                            if output_format == "json":
                                output = json.loads(result.stdout)
                                self.assertEqual(output["status"], status)
                                if name == "zero":
                                    self.assertEqual(
                                        output["failures"],
                                        ["unassessable phase coverage missing"],
                                    )
                            else:
                                self.assertIn(
                                    f"# Framework calibration: {status}", result.stdout
                                )
                                if name == "zero":
                                    self.assertIn(
                                        "unassessable phase coverage missing",
                                        result.stdout,
                                    )

    def test_representative_synthetic_fixture_passes(self):
        result = CAL.analyze(self.fixture("passing-synthetic.json"))
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["counts"]["sessions"], 30)
        self.assertGreaterEqual(result["metrics"]["interRaterQwk"], 0.67)
        self.assertLessEqual(result["metrics"]["modelHumanMae"], 10)

    def test_biased_fixture_fails_and_names_subgroup_gap(self):
        result = CAL.analyze(self.fixture("failing-synthetic.json"))
        self.assertEqual(result["status"], "NOT_CALIBRATED")
        self.assertTrue(
            any("language MAE gap" in failure for failure in result["failures"])
        )

    def test_version_must_be_a_real_integer(self):
        # `True != 1` and `1.0 != 1` are both false, so a bare inequality
        # accepted a boolean and a float where the contract keys beside it
        # already refused them.
        for bogus in (True, 1.0):
            raw = self.fixture("passing-synthetic.json")
            raw["version"] = bogus
            with self.assertRaisesRegex(ValueError, r"\$\.version"):
                CAL.analyze(raw)

    def test_a_cohort_too_small_to_compare_is_named(self):
        # Cohorts under the size threshold were dropped from the eligible set
        # and from the result, so a corpus that never gathered enough of one
        # read exactly like a corpus with no subgroup gap.
        result = CAL.analyze(self.fixture("passing-synthetic.json"))
        for key in ("language", "accentCohort", "transcriptQuality"):
            self.assertIn("insufficientCohorts", result["metrics"]["subgroups"][key])

    def test_malformed_fixture_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "unique"):
            CAL.analyze(self.fixture("malformed-synthetic.json"))

    def test_sparse_valid_input_fails_phase_band_and_hint_coverage(self):
        sparse = self.fixture("passing-synthetic.json")
        sparse["samples"] = sparse["samples"][:1]
        sparse["samples"][0]["hintsUsed"] = 0
        for phase in CAL.PHASES[1:]:
            sparse["samples"][0]["modelScores"][phase] = None
            for rating in sparse["samples"][0]["humanRatings"]:
                rating["scores"][phase] = None
        failures = CAL.analyze(sparse)["failures"]
        self.assertTrue(any("phase coverage" in failure for failure in failures))
        self.assertTrue(any("score-band coverage" in failure for failure in failures))
        self.assertTrue(any("hint coverage" in failure for failure in failures))

    def test_blank_template_truthfully_fails_coverage(self):
        result = CAL.analyze(self.fixture("blinded-review-template.json"))
        self.assertEqual(result["status"], "NOT_CALIBRATED")
        self.assertIn("sessions 0 < 30", result["failures"])

    def test_checked_in_status_does_not_claim_real_calibration(self):
        status = (ROOT / "docs" / "rubric-calibration.md").read_text()
        self.assertIn("Status: **NOT CALIBRATED**", status)
        self.assertIn(
            "synthetic analyzer fixtures are tests, not evidence",
            " ".join(status.split()),
        )


if __name__ == "__main__":
    unittest.main()
