import unittest
from pathlib import Path

from scripts.package_release import profile_for


class ReleaseProfileTests(unittest.TestCase):
    def test_linux_and_windows_wheels_have_platform_profiles(self):
        linux = profile_for(
            Path("mnemokernel_native-0.1.1-cp310-abi3-manylinux_2_34_x86_64.whl")
        )
        windows = profile_for(
            Path("mnemokernel_native-0.1.1-cp310-abi3-win_amd64.whl")
        )
        self.assertEqual(linux.archive_suffix, "linux-x86_64")
        self.assertEqual(windows.archive_suffix, "windows-x86_64")
        self.assertTrue(windows.bundled_native_members[-1].endswith(".pyd"))

    def test_unknown_wheel_target_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "supported targets"):
            profile_for(
                Path("mnemokernel_native-0.1.1-cp310-abi3-manylinux_aarch64.whl")
            )


if __name__ == "__main__":
    unittest.main()
