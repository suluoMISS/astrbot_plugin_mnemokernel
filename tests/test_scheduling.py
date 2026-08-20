import unittest
from datetime import datetime
from zoneinfo import ZoneInfo

from mnemokernel_adapter.scheduling import due_journal_date


class SchedulingTests(unittest.TestCase):
    def test_after_trigger_summarizes_yesterday(self):
        now = datetime(2026, 8, 20, 2, 0, tzinfo=ZoneInfo("Asia/Shanghai"))
        self.assertEqual(due_journal_date(now, 2, 0).isoformat(), "2026-08-19")

    def test_before_trigger_uses_latest_elapsed_window(self):
        now = datetime(2026, 8, 20, 1, 59, tzinfo=ZoneInfo("Asia/Shanghai"))
        self.assertEqual(due_journal_date(now, 2, 0).isoformat(), "2026-08-18")

    def test_naive_datetime_is_rejected(self):
        with self.assertRaises(ValueError):
            due_journal_date(datetime(2026, 8, 20, 2, 0), 2, 0)


if __name__ == "__main__":
    unittest.main()
