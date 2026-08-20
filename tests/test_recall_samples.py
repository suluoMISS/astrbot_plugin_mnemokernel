import json
import unittest
from pathlib import Path


class RecallSampleTests(unittest.TestCase):
    def test_call_judgment_sample_set_has_three_explicit_buckets(self):
        path = Path(__file__).parent / "fixtures" / "recall_call_samples.json"
        samples = json.loads(path.read_text(encoding="utf-8"))
        self.assertGreaterEqual(len(samples), 6)
        buckets = {sample["class"] for sample in samples}
        self.assertEqual(buckets, {"must_call", "must_not_call", "clarify"})
        self.assertEqual(len({sample["id"] for sample in samples}), len(samples))
        for sample in samples:
            self.assertTrue(sample["user"].strip())
            self.assertTrue(sample["reason"].strip())


if __name__ == "__main__":
    unittest.main()
