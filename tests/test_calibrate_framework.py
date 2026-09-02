import importlib.util
import json
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
