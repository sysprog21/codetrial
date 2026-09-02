import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "gen_problems", ROOT / "scripts/gen-problems.py"
)
GEN = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(GEN)


class ProblemMetadataGeneratorTests(unittest.TestCase):
    def source_with(self, problems):
        temporary = tempfile.TemporaryDirectory()
        path = Path(temporary.name) / "problems.json"
        path.write_text(json.dumps(problems))
        return temporary, patch.object(GEN, "SOURCE", path)

    def test_rejects_duplicate_ids_and_invalid_topic_lists(self):
        valid = {"id": "one", "difficulty": "Easy", "topics": ["Array"]}
        for problems in (
            [valid, valid],
            [{**valid, "topics": []}],
            [{**valid, "topics": ["Array", "Array"]}],
            [{**valid, "difficulty": "Extreme"}],
        ):
            temporary, source = self.source_with(problems)
            with temporary, source, self.assertRaises(RuntimeError):
                GEN.validated_problems()

    def test_public_projection_has_a_closed_schema_and_ordered_stages(self):
        problem = {"id": "one", "difficulty": "Medium", "topics": ["Array"]}
        metadata = GEN.public_metadata(problem)
        self.assertEqual(set(metadata), GEN.PUBLIC_METADATA_KEYS)
        self.assertEqual(metadata["reactoStages"], list(GEN.REACTO_STAGES))
        self.assertEqual(
            [item["stage"] for item in metadata["followUpDirections"]],
            list(GEN.REACTO_STAGES),
        )
        serialized = json.dumps(metadata).lower()
        for forbidden in ("optimal", "pitfall", "hintladder", "expecteddiscussion"):
            self.assertNotIn(forbidden, serialized)


if __name__ == "__main__":
    unittest.main()
