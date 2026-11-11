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
STAR_PHASES = ("Situation", "Task", "Action", "Result")


class CalibrationTests(unittest.TestCase):
    def fixture(self, name):
        return json.loads(
            (ROOT / "tests" / "fixtures" / "calibration" / name).read_text()
        )

    def applicable_scored_fixture(self):
        raw = self.fixture("passing-synthetic.json")
        for sample in raw["samples"]:
            rows = [sample["modelScores"]] + [
                rating["scores"] for rating in sample["humanRatings"]
            ]
            for row in rows:
                for phase in CAL.PHASES:
                    if sample["loop"] == "coding_only" and phase in STAR_PHASES:
                        continue
                    if row[phase] is None:
                        row[phase] = 82
        return raw

    def test_unassessable_coverage_zero_and_one(self):
        raw = self.applicable_scored_fixture()
        uncovered = CAL.analyze(raw)
        self.assertEqual(uncovered["status"], "NOT_CALIBRATED")
        self.assertEqual(uncovered["failures"], ["unassessable phase coverage missing"])
        for sample_index, phase in itertools.product(
            range(len(raw["samples"])), CAL.PHASES
        ):
            if (
                raw["samples"][sample_index]["loop"] == "coding_only"
                and phase in STAR_PHASES
            ):
                continue
            with self.subTest(sample=sample_index, phase=phase):
                one = copy.deepcopy(raw)
                sample = one["samples"][sample_index]
                sample["modelScores"][phase] = None
                for rating in sample["humanRatings"]:
                    rating["scores"][phase] = None
                covered = CAL.analyze(one)
                self.assertEqual(covered["status"], "PASS", covered["failures"])
                self.assertEqual(
                    covered["counts"]["assessedPhasePairs"],
                    uncovered["counts"]["assessedPhasePairs"] - 1,
                )

    def test_unassessable_coverage_with_extra_session(self):
        raw = self.applicable_scored_fixture()
        extra = copy.deepcopy(raw["samples"][0])
        extra["id"] = "extra-session"
        raw["samples"].append(extra)
        uncovered = CAL.analyze(raw)
        self.assertEqual(uncovered["failures"], ["unassessable phase coverage missing"])
        extra["modelScores"]["Repeat"] = None
        for rating in extra["humanRatings"]:
            rating["scores"]["Repeat"] = None
        covered = CAL.analyze(raw)
        self.assertEqual(covered["status"], "PASS", covered["failures"])
        self.assertEqual(
            covered["counts"]["assessedPhasePairs"],
            uncovered["counts"]["assessedPhasePairs"] - 1,
        )

    def test_passing_fixture_has_applicable_unassessable_phase(self):
        raw = self.fixture("passing-synthetic.json")
        self.assertTrue(
            any(
                sample["modelScores"][phase] is None
                and all(
                    rating["scores"][phase] is None for rating in sample["humanRatings"]
                )
                for sample in raw["samples"]
                for phase in CAL.PHASES
                if sample["loop"] == "coding_behavioral" or phase not in STAR_PHASES
            )
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
            reviewer_pattern = (
                "reviewer-c" if model_score is not None else "reviewer-[ab]"
            )
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
                    with self.assertRaisesRegex(
                        ValueError, rf"assessability mismatch \({reviewer_pattern}\)$"
                    ):
                        CAL.analyze(raw)

    def test_coding_only_star_scores_must_be_null(self):
        for phase, value, source in itertools.product(
            STAR_PHASES, (0, 50, 100), ("model", "human", "both", "partial")
        ):
            with self.subTest(phase=phase, value=value, source=source):
                raw = self.fixture("passing-synthetic.json")
                sample = raw["samples"][0]
                if source in ("model", "both"):
                    sample["modelScores"][phase] = value
                if source in ("human", "both"):
                    for rating in sample["humanRatings"]:
                        rating["scores"][phase] = value
                if source == "partial":
                    for rating in sample["humanRatings"]:
                        del rating["scores"][phase]
                    sample["humanRatings"].append(
                        {"reviewerId": "reviewer-c", "scores": {phase: value}}
                    )
                with self.assertRaisesRegex(
                    ValueError, "not applicable to coding_only"
                ):
                    CAL.analyze(raw)
                if source == "both":
                    sample["loop"] = "coding_behavioral"
                    result = CAL.analyze(raw)
                    self.assertEqual(result["status"], "PASS", result["failures"])

    def test_inapplicable_star_judgments_may_be_omitted(self):
        for raw, status, failures in (
            (self.fixture("passing-synthetic.json"), "PASS", []),
            (
                self.applicable_scored_fixture(),
                "NOT_CALIBRATED",
                ["unassessable phase coverage missing"],
            ),
        ):
            expected_pairs = sum(
                value is not None
                for sample in raw["samples"]
                for value in sample["modelScores"].values()
            )
            for recorded in (0, 1, 2):
                with self.subTest(status=status, recorded=recorded):
                    modified = copy.deepcopy(raw)
                    for sample in modified["samples"]:
                        if sample["loop"] == "coding_only":
                            for rating in sample["humanRatings"][recorded:]:
                                for phase in STAR_PHASES:
                                    del rating["scores"][phase]
                    result = CAL.analyze(modified)
                    self.assertEqual(result["status"], status, result["failures"])
                    self.assertEqual(result["failures"], failures)
                    self.assertEqual(
                        result["counts"]["assessedPhasePairs"], expected_pairs
                    )

    def test_inapplicable_model_phase_keys_remain_required(self):
        for phase in STAR_PHASES:
            with self.subTest(phase=phase):
                raw = self.fixture("passing-synthetic.json")
                del raw["samples"][0]["modelScores"][phase]
                with self.assertRaisesRegex(ValueError, "modelScores: missing="):
                    CAL.analyze(raw)

    def test_recorded_count_includes_partial_adjudicator(self):
        raw = self.fixture("passing-synthetic.json")
        baseline = CAL.analyze(raw)
        sample = raw["samples"][0]
        original = sample["humanRatings"]
        model_score = sample["modelScores"]["Repeat"]
        old_error = abs(
            model_score - sum(rating["scores"]["Repeat"] for rating in original) / 2
        )
        remaining_score = original[1]["scores"]["Repeat"]
        del original[0]["scores"]["Repeat"]
        third = {"reviewerId": "reviewer-c", "scores": {"Repeat": 90}}
        expected_error = abs(
            model_score - (remaining_score + third["scores"]["Repeat"]) / 2
        )
        for ratings in itertools.permutations([*original, third]):
            with self.subTest(order=[rating["reviewerId"] for rating in ratings]):
                sample["humanRatings"] = list(ratings)
                result = CAL.analyze(raw)
                self.assertEqual(result["status"], "PASS", result["failures"])
                self.assertEqual(result["counts"], baseline["counts"])
                self.assertAlmostEqual(
                    result["metrics"]["modelHumanMae"],
                    baseline["metrics"]["modelHumanMae"]
                    + (expected_error - old_error)
                    / baseline["counts"]["assessedPhasePairs"],
                )

    def test_partial_adjudicator_only_contributes_recorded_scores(self):
        raw = self.fixture("passing-synthetic.json")
        sample = raw["samples"][0]
        # Filling an omitted phase with the model score must change the metrics.
        for rating in sample["humanRatings"]:
            rating["scores"]["Example"] = 50
        baseline = CAL.analyze(raw)
        third = {"reviewerId": "reviewer-c", "scores": {"Repeat": 90}}
        for ratings in itertools.permutations([*sample["humanRatings"], third]):
            with self.subTest(order=[rating["reviewerId"] for rating in ratings]):
                sample["humanRatings"] = list(ratings)
                result = CAL.analyze(raw)
                self.assertEqual(result["status"], "PASS", result["failures"])
                self.assertEqual(result["counts"], baseline["counts"])
                pairs = baseline["counts"]["assessedPhasePairs"]
                self.assertAlmostEqual(
                    result["metrics"]["modelHumanMae"],
                    baseline["metrics"]["modelHumanMae"] + 20 / pairs,
                )
                self.assertAlmostEqual(
                    result["metrics"]["bandAgreement"],
                    baseline["metrics"]["bandAgreement"] - 1 / pairs,
                )
                self.assertLess(
                    result["metrics"]["interRaterQwk"],
                    baseline["metrics"]["interRaterQwk"],
                )

    def test_partial_adjudicator_omissions_and_explicit_null_differ(self):
        for model_score in (30, None):
            raw = self.fixture("passing-synthetic.json")
            sample = raw["samples"][0]
            sample["modelScores"]["Repeat"] = model_score
            for rating in sample["humanRatings"]:
                rating["scores"]["Repeat"] = model_score
            third = {"reviewerId": "reviewer-c", "scores": {"Algorithm": 30}}
            for ratings in itertools.permutations([*sample["humanRatings"], third]):
                with self.subTest(
                    model=model_score, order=[r["reviewerId"] for r in ratings]
                ):
                    sample["humanRatings"] = list(ratings)
                    result = CAL.analyze(raw)
                    self.assertEqual(result["status"], "PASS", result["failures"])
                    third["scores"]["Repeat"] = None if model_score is not None else 30
                    with self.assertRaisesRegex(
                        ValueError, r"assessability mismatch \(reviewer-c\)$"
                    ):
                        CAL.analyze(raw)
                    del third["scores"]["Repeat"]

    def test_every_phase_needs_two_recorded_human_ratings(self):
        for phase, model_score, recorded in itertools.product(
            CAL.PHASES, (50, None), (0, 1)
        ):
            with self.subTest(phase=phase, model=model_score, recorded=recorded):
                raw = self.fixture("passing-synthetic.json")
                sample = raw["samples"][1]
                sample["modelScores"][phase] = model_score
                for rating in sample["humanRatings"]:
                    del rating["scores"][phase]
                if recorded:
                    sample["humanRatings"].append(
                        {"reviewerId": "reviewer-c", "scores": {phase: model_score}}
                    )
                with self.assertRaisesRegex(
                    ValueError, rf"{phase}: need at least two human ratings"
                ):
                    CAL.analyze(raw)

    def test_partial_scores_still_validate_keys_types_and_ranges(self):
        for scores in (
            None,
            [],
            {"Unknown": 30},
            {"Repeat": True},
            {"Repeat": 1.5},
            {"Repeat": -1},
            {"Repeat": 101},
        ):
            with self.subTest(scores=scores):
                raw = self.fixture("passing-synthetic.json")
                raw["samples"][0]["humanRatings"].append(
                    {"reviewerId": "reviewer-c", "scores": scores}
                )
                with self.assertRaises(ValueError):
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
        zero = self.applicable_scored_fixture()
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
        partial = self.fixture("passing-synthetic.json")
        partial["samples"][0]["humanRatings"].append(
            {"reviewerId": "reviewer-c", "scores": {"Repeat": 90}}
        )
        insufficient = copy.deepcopy(partial)
        for rating in insufficient["samples"][0]["humanRatings"][:2]:
            del rating["scores"]["Repeat"]
        inapplicable = self.fixture("passing-synthetic.json")
        sample = inapplicable["samples"][0]
        sample["modelScores"]["Situation"] = 80
        for rating in sample["humanRatings"]:
            rating["scores"]["Situation"] = 80
        omitted = self.fixture("passing-synthetic.json")
        for sample in omitted["samples"]:
            if sample["loop"] == "coding_only":
                for rating in sample["humanRatings"]:
                    for phase in STAR_PHASES:
                        del rating["scores"][phase]
        inapplicable_human = copy.deepcopy(omitted)
        inapplicable_human["samples"][0]["humanRatings"][0]["scores"]["Situation"] = 80
        cases = (
            ("control", self.fixture("passing-synthetic.json"), 0, "PASS"),
            ("zero", zero, 1, "NOT_CALIBRATED"),
            ("one", one, 0, "PASS"),
            ("mixed", mixed, 2, None),
            ("partial", partial, 0, "PASS"),
            ("insufficient", insufficient, 2, None),
            ("inapplicable", inapplicable, 2, None),
            ("omitted", omitted, 0, "PASS"),
            ("inapplicable-human", inapplicable_human, 2, None),
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
                            if name == "insufficient":
                                self.assertIn(
                                    "Repeat: need at least two human ratings",
                                    result.stderr,
                                )
                            elif name.startswith("inapplicable"):
                                self.assertIn(
                                    "Situation: not applicable to coding_only",
                                    result.stderr,
                                )
                            else:
                                self.assertIn("assessability mismatch", result.stderr)
                                self.assertIn("(reviewer-c)", result.stderr)
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
