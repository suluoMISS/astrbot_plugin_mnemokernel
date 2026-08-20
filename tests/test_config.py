import unittest

from mnemokernel_adapter.config import MnemoKernelConfig


class ConfigTests(unittest.TestCase):
    def test_defaults_are_conservative(self):
        config = MnemoKernelConfig.from_astrbot_config({})
        self.assertTrue(config.enabled)
        self.assertFalse(config.capture.enabled)
        self.assertFalse(config.capture.permits("group", "g-1"))
        self.assertFalse(config.recall.enabled)
        self.assertFalse(config.diary.enabled)
        self.assertFalse(config.privacy.permits_control("group", "user-1"))
        self.assertEqual(config.diary.timezone, "Asia/Shanghai")
        self.assertEqual(config.retention_days, 14)

    def test_diary_configuration_is_bounded(self):
        config = MnemoKernelConfig.from_astrbot_config(
            {
                "diary": {
                    "enabled": True,
                    "timezone": "UTC",
                    "daily_hour": 99,
                    "daily_minute": -4,
                }
            }
        )
        self.assertTrue(config.diary.enabled)
        self.assertEqual(config.diary.timezone, "UTC")
        self.assertEqual(config.diary.daily_hour, 23)
        self.assertEqual(config.diary.daily_minute, 0)

    def test_group_capture_requires_allowlist(self):
        config = MnemoKernelConfig.from_astrbot_config(
            {
                "capture": {
                    "enabled": True,
                    "capture_private": False,
                    "group_allowlist": ["g-1", "g-1", ""],
                }
            }
        )
        self.assertTrue(config.capture.permits("group", "g-1"))
        self.assertFalse(config.capture.permits("group", "g-2"))
        self.assertFalse(config.capture.permits("private", "u-1"))

    def test_group_control_requires_sender_allowlist(self):
        config = MnemoKernelConfig.from_astrbot_config(
            {"privacy": {"group_controller_ids": ["admin-1", "admin-1", ""]}}
        )
        self.assertTrue(config.privacy.permits_control("private", "any-user"))
        self.assertTrue(config.privacy.permits_control("group", "admin-1"))
        self.assertFalse(config.privacy.permits_control("group", "member-1"))

    def test_database_path_cannot_escape_plugin_data_directory(self):
        config = MnemoKernelConfig.from_astrbot_config(
            {"storage": {"database_filename": "../outside.sqlite3"}}
        )
        self.assertEqual(config.database_filename, "mnemokernel.sqlite3")

    def test_retention_days_are_bounded(self):
        config = MnemoKernelConfig.from_astrbot_config(
            {"storage": {"retention_days": 99_999}}
        )
        self.assertEqual(config.retention_days, 3_650)


if __name__ == "__main__":
    unittest.main()

