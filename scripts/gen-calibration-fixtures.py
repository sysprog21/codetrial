#!/usr/bin/env python3
"""Generate deterministic synthetic calibration fixtures (never production evidence)."""

import argparse
import copy
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "tests" / "fixtures" / "calibration"
PHASES = (
    "Repeat",
    "Example",
    "Algorithm",
    "Coding",
    "Test",
    "Optimizations",
    "Situation",
    "Task",
    "Action",
    "Result",
)


def scores(base, coding_only=False):
    return {
        phase: None
        if coding_only and phase in ("Situation", "Task", "Action", "Result")
        else base
        for phase in PHASES
    }


def corpus():
    samples = []
    bases = (30, 50, 68, 82, 95)
    for index in range(30):
        coding_only = index % 2 == 0
        base = bases[index % len(bases)]
        model = scores(base, coding_only)
        samples.append(
            {
                "id": f"blind-{index + 1:03d}",
                "difficulty": ("Easy", "Medium", "Hard")[index % 3],
                "language": ("python", "javascript", "cpp")[index % 3],
                "loop": "coding_only" if coding_only else "coding_behavioral",
                "hintsUsed": index % 3,
                "transcriptQuality": ("clear", "degraded", "missing")[index % 3],
                "accentCohort": ("cohort-a", "cohort-b", "cohort-c")[index % 3],
                "modelScores": model,
                "humanRatings": [
                    {
                        "reviewerId": "reviewer-a",
                        "scores": scores(max(0, base - 1), coding_only),
                    },
                    {
                        "reviewerId": "reviewer-b",
                        "scores": scores(min(100, base + 1), coding_only),
                    },
                ],
            }
        )
    return {
        "version": 1,
        "contract": {
            "bundleVersion": 4,
            "reportPromptVersion": 4,
            "rubricVersion": 1,
            "reportSchemaVersion": 1,
            "model": "synthetic-test-only",
        },
        "samples": samples,
    }


def content():
    passing = corpus()
    failing = copy.deepcopy(passing)
    for sample in failing["samples"]:
        if sample["language"] == "cpp":
            for phase, value in sample["modelScores"].items():
                if value is not None:
                    sample["modelScores"][phase] = min(100, value + 30)
    malformed = copy.deepcopy(passing)
    malformed["samples"][1]["id"] = malformed["samples"][0]["id"]
    template = {
        "version": 1,
        "contract": {
            "bundleVersion": 4,
            "reportPromptVersion": 4,
            "rubricVersion": 1,
            "reportSchemaVersion": 1,
            "model": "FILL-ME",
        },
        "samples": [],
    }
    return {
        "passing-synthetic.json": passing,
        "failing-synthetic.json": failing,
        "malformed-synthetic.json": malformed,
        "blinded-review-template.json": template,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for name, value in content().items():
        rendered = json.dumps(value, indent=2, ensure_ascii=False) + "\n"
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != rendered:
                stale.append(str(path.relative_to(ROOT)))
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(rendered, encoding="utf-8")
    if stale:
        raise SystemExit("stale calibration fixtures: " + ", ".join(stale))


if __name__ == "__main__":
    main()
