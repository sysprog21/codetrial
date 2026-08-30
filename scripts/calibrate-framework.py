#!/usr/bin/env python3
"""Validate blinded framework ratings and calculate the calibration release gate."""

import argparse
import itertools
import json
import sys
from collections import defaultdict
from pathlib import Path

PHASES = ("Repeat", "Example", "Algorithm", "Coding", "Test", "Optimizations",
          "Situation", "Task", "Action", "Result")
BANDS = ((0, 39), (40, 59), (60, 74), (75, 89), (90, 100))
THRESHOLDS = {"sessions": 30, "interRaterQwk": 0.67, "modelHumanMae": 10.0,
              "bandAgreement": 0.80, "subgroupMaeGap": 5.0, "cohortSize": 5}


def exact_keys(value, expected, path):
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected object")
    unknown = sorted(set(value) - set(expected))
    missing = sorted(set(expected) - set(value))
    if missing or unknown:
        raise ValueError(f"{path}: missing={missing} unknown={unknown}")


def score(value, path):
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 100:
        raise ValueError(f"{path}: expected integer 0..100 or null")
    return value


def band(value):
    return next(index for index, (low, high) in enumerate(BANDS) if low <= value <= high)


def qwk(left, right):
    if len(left) != len(right) or not left:
        return None
    observed = [[0] * 5 for _ in range(5)]
    left_counts, right_counts = [0] * 5, [0] * 5
    for a, b in zip(left, right):
        x, y = band(a), band(b)
        observed[x][y] += 1
        left_counts[x] += 1
        right_counts[y] += 1
    weighted_observed = sum(((i - j) ** 2 / 16) * observed[i][j]
                            for i in range(5) for j in range(5))
    weighted_expected = sum(((i - j) ** 2 / 16) * left_counts[i] * right_counts[j] / len(left)
                            for i in range(5) for j in range(5))
    if weighted_expected == 0:
        return 1.0 if weighted_observed == 0 else 0.0
    return 1 - weighted_observed / weighted_expected


def mean(values):
    return sum(values) / len(values) if values else None


def analyze(raw):
    exact_keys(raw, ["version", "contract", "samples"], "$")
    if raw["version"] != 1:
        raise ValueError("$.version: expected 1")
    exact_keys(raw["contract"], ["bundleVersion", "reportPromptVersion", "rubricVersion",
                                 "reportSchemaVersion", "model"], "$.contract")
    contract = raw["contract"]
    for key in ("bundleVersion", "reportPromptVersion", "rubricVersion", "reportSchemaVersion"):
        if isinstance(contract[key], bool) or not isinstance(contract[key], int) or contract[key] < 1:
            raise ValueError(f"$.contract.{key}: expected positive integer")
    if not isinstance(contract["model"], str) or not contract["model"].strip():
        raise ValueError("$.contract.model: expected nonempty model identity")
    samples = raw["samples"]
    if not isinstance(samples, list):
        raise ValueError("$.samples: expected array")
    ids, pairs, errors, strata = set(), [], defaultdict(list), defaultdict(set)
    assessed_phases, observed_bands, hint_strata = set(), set(), set()
    pairwise = defaultdict(lambda: [[], []])
    for index, sample in enumerate(samples):
        path = f"$.samples[{index}]"
        exact_keys(sample, ["id", "difficulty", "language", "loop", "hintsUsed",
                            "transcriptQuality", "accentCohort", "modelScores", "humanRatings"], path)
        sample_id = sample["id"]
        if not isinstance(sample_id, str) or not sample_id or sample_id in ids:
            raise ValueError(f"{path}.id: expected unique nonempty string")
        ids.add(sample_id)
        if sample["difficulty"] not in ("Easy", "Medium", "Hard"):
            raise ValueError(f"{path}.difficulty: invalid stratum")
        if sample["loop"] not in ("coding_only", "coding_behavioral"):
            raise ValueError(f"{path}.loop: invalid stratum")
        if sample["transcriptQuality"] not in ("clear", "degraded", "missing"):
            raise ValueError(f"{path}.transcriptQuality: invalid stratum")
        if not isinstance(sample["language"], str) or not sample["language"]:
            raise ValueError(f"{path}.language: expected nonempty string")
        if sample["accentCohort"] is not None and (not isinstance(sample["accentCohort"], str)
                                                    or not sample["accentCohort"]):
            raise ValueError(f"{path}.accentCohort: expected string or null")
        if isinstance(sample["hintsUsed"], bool) or not isinstance(sample["hintsUsed"], int) or sample["hintsUsed"] < 0:
            raise ValueError(f"{path}.hintsUsed: expected nonnegative integer")
        hint_strata.add("zero" if sample["hintsUsed"] == 0 else "nonzero")
        exact_keys(sample["modelScores"], PHASES, f"{path}.modelScores")
        model = {phase: score(sample["modelScores"][phase], f"{path}.modelScores.{phase}") for phase in PHASES}
        ratings = sample["humanRatings"]
        if not isinstance(ratings, list) or len(ratings) < 2:
            raise ValueError(f"{path}.humanRatings: need at least two blinded ratings")
        reviewers, human = set(), defaultdict(list)
        for rating_index, rating in enumerate(ratings):
            rating_path = f"{path}.humanRatings[{rating_index}]"
            exact_keys(rating, ["reviewerId", "scores"], rating_path)
            reviewer = rating["reviewerId"]
            if not isinstance(reviewer, str) or not reviewer or reviewer in reviewers:
                raise ValueError(f"{rating_path}.reviewerId: expected distinct pseudonym")
            reviewers.add(reviewer)
            exact_keys(rating["scores"], PHASES, f"{rating_path}.scores")
            for phase in PHASES:
                value = score(rating["scores"][phase], f"{rating_path}.scores.{phase}")
                if value is not None:
                    human[phase].append((reviewer, value))
        for phase in PHASES:
            values = human[phase]
            if model[phase] is None:
                if values:
                    raise ValueError(f"{path}.{phase}: model/human assessability mismatch")
                continue
            if len(values) < 2:
                raise ValueError(f"{path}.{phase}: assessed phase needs two human raters")
            assessed_phases.add(phase)
            observed_bands.add(band(model[phase]))
            human_mean = mean([value for _, value in values])
            error = abs(model[phase] - human_mean)
            pairs.append((model[phase], human_mean))
            for key in ("language", "transcriptQuality", "accentCohort"):
                group = sample[key]
                if group is not None:
                    errors[(key, group)].append(error)
            for left, right in itertools.combinations(sorted(values), 2):
                pairwise[phase][0].append(left[1])
                pairwise[phase][1].append(right[1])
        for key in ("difficulty", "language", "loop", "transcriptQuality"):
            strata[key].add(sample[key])

    failures = []
    if len(samples) < THRESHOLDS["sessions"]:
        failures.append(f"sessions {len(samples)} < {THRESHOLDS['sessions']}")
    required = {"difficulty": {"Easy", "Medium", "Hard"},
                "loop": {"coding_only", "coding_behavioral"},
                "transcriptQuality": {"clear", "degraded", "missing"}}
    for key, wanted in required.items():
        if not wanted <= strata[key]:
            failures.append(f"{key} coverage missing {sorted(wanted - strata[key])}")
    if len(strata["language"]) < 3:
        failures.append("language coverage needs at least 3 languages")
    if assessed_phases != set(PHASES):
        failures.append(f"phase coverage missing {sorted(set(PHASES) - assessed_phases)}")
    if observed_bands != set(range(len(BANDS))):
        failures.append(f"score-band coverage missing {sorted(set(range(len(BANDS))) - observed_bands)}")
    if hint_strata != {"zero", "nonzero"}:
        failures.append("hint coverage needs zero and nonzero use")
    agreements = [qwk(*values) for values in pairwise.values() if values[0]]
    inter_rater = mean(agreements)
    mae = mean([abs(model - human) for model, human in pairs])
    band_agreement = mean([1.0 if band(model) == band(round(human)) else 0.0 for model, human in pairs])
    if inter_rater is None or inter_rater < THRESHOLDS["interRaterQwk"]:
        failures.append(f"inter-rater QWK {inter_rater} < {THRESHOLDS['interRaterQwk']}")
    if mae is None or mae > THRESHOLDS["modelHumanMae"]:
        failures.append(f"model/human MAE {mae} > {THRESHOLDS['modelHumanMae']}")
    if band_agreement is None or band_agreement < THRESHOLDS["bandAgreement"]:
        failures.append(f"band agreement {band_agreement} < {THRESHOLDS['bandAgreement']}")
    subgroup = {}
    for key in ("language", "accentCohort", "transcriptQuality"):
        eligible = {group: mean(values) for (kind, group), values in errors.items()
                    if kind == key and len(values) >= THRESHOLDS["cohortSize"]}
        gap = max(eligible.values()) - min(eligible.values()) if len(eligible) >= 2 else None
        subgroup[key] = {"eligibleCohorts": eligible, "maxMaeGap": gap}
        if gap is not None and gap > THRESHOLDS["subgroupMaeGap"]:
            failures.append(f"{key} MAE gap {gap} > {THRESHOLDS['subgroupMaeGap']}")
    return {"status": "PASS" if not failures else "NOT_CALIBRATED", "contract": contract,
            "counts": {"sessions": len(samples), "assessedPhasePairs": len(pairs)},
            "metrics": {"interRaterQwk": inter_rater, "modelHumanMae": mae,
                        "bandAgreement": band_agreement, "subgroups": subgroup},
            "thresholds": THRESHOLDS, "failures": failures}


def markdown(result):
    metrics = result["metrics"]
    lines = [f"# Framework calibration: {result['status']}", "",
             f"Sessions: {result['counts']['sessions']}; assessed phase pairs: {result['counts']['assessedPhasePairs']}.", "",
             f"- Inter-rater QWK: {metrics['interRaterQwk']}",
             f"- Model/human MAE: {metrics['modelHumanMae']}",
             f"- Rubric-band agreement: {metrics['bandAgreement']}"]
    if result["failures"]:
        lines += ["", "## Failed gates", ""] + [f"- {item}" for item in result["failures"]]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("--format", choices=("json", "markdown"), default="json")
    args = parser.parse_args()
    try:
        result = analyze(json.loads(args.input.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"calibration input invalid: {error}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2, sort_keys=True) if args.format == "json" else markdown(result), end="\n" if args.format == "json" else "")
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
