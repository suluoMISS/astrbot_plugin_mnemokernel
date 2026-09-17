"""Build a deterministic AstrBot upload archive around one verified wheel."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
PLUGIN_NAME = "astrbot_plugin_mnemokernel"
VERSION = "v0.1.1"


@dataclass(frozen=True, slots=True)
class ReleaseProfile:
    wheel_prefix: str
    archive_suffix: str
    platform_label: str
    bundled_native_members: tuple[str, ...]


RELEASE_PROFILES = (
    ReleaseProfile(
        wheel_prefix="mnemokernel_native-0.1.1-cp310-abi3-manylinux_2_34_x86_64",
        archive_suffix="linux-x86_64",
        platform_label="Linux x86_64",
        bundled_native_members=(
            "_mnemokernel/__init__.py",
            "_mnemokernel/_mnemokernel.abi3.so",
        ),
    ),
    ReleaseProfile(
        wheel_prefix="mnemokernel_native-0.1.1-cp310-abi3-win_amd64",
        archive_suffix="windows-x86_64",
        platform_label="Windows x86_64",
        bundled_native_members=(
            "_mnemokernel/__init__.py",
            "_mnemokernel/_mnemokernel.pyd",
        ),
    ),
)

ROOT_FILES = (
    "__init__.py",
    "main.py",
    "metadata.yaml",
    "logo.png",
    "_conf_schema.json",
    "README.md",
    "SECURITY.md",
    "CHANGELOG.md",
    "LICENSE",
)
TREE_PATTERNS = (
    ("pages", "*/*.html"),
    ("pages", "*/*.js"),
    ("pages", "*/*.css"),
    (".astrbot-plugin/i18n", "*.json"),
    ("mnemokernel_adapter", "*.py"),
    ("migrations", "*.sql"),
    ("schemas", "*.json"),
)


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def archive_info(relative: PurePosixPath) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(str(PurePosixPath(PLUGIN_NAME) / relative))
    info.date_time = (1980, 1, 1, 0, 0, 0)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    info.create_system = 3
    return info


def source_files() -> list[tuple[PurePosixPath, Path]]:
    files = [(PurePosixPath(name), ROOT / name) for name in ROOT_FILES]
    for directory, pattern in TREE_PATTERNS:
        files.extend(
            (PurePosixPath(path.relative_to(ROOT).as_posix()), path)
            for path in sorted((ROOT / directory).glob(pattern))
        )
    missing = [str(path) for _, path in files if not path.is_file()]
    if missing:
        raise RuntimeError(f"release input is missing: {missing}")
    return sorted(files, key=lambda item: str(item[0]))


def profile_for(wheel: Path) -> ReleaseProfile:
    for profile in RELEASE_PROFILES:
        if wheel.name.startswith(profile.wheel_prefix):
            return profile
    supported = ", ".join(profile.platform_label for profile in RELEASE_PROFILES)
    raise RuntimeError(f"unexpected release wheel: {wheel}; supported targets: {supported}")


def bundled_native_files(
    wheel: Path, profile: ReleaseProfile
) -> list[tuple[PurePosixPath, bytes]]:
    with zipfile.ZipFile(wheel) as archive:
        files = []
        for member in profile.bundled_native_members:
            try:
                payload = archive.read(member)
            except KeyError as exc:
                raise RuntimeError(f"release wheel is missing bundled runtime file: {member}") from exc
            files.append((PurePosixPath("native_runtime") / member, payload))
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path)
    parser.add_argument("output_directory", type=Path)
    args = parser.parse_args()

    wheel = args.wheel.resolve()
    if not wheel.is_file():
        raise RuntimeError(f"unexpected release wheel: {wheel}")
    profile = profile_for(wheel)
    output = args.output_directory.resolve()
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{PLUGIN_NAME}-{VERSION}-{profile.archive_suffix}.zip"
    wheel_copy = output / wheel.name

    install_note = (
        f"MnemoKernel 0.1.1 release target: {profile.platform_label}, Python >= 3.10.\n"
        "Upload this ZIP in AstrBot. The ABI3 runtime is bundled under "
        "native_runtime/, so AstrBot does not need to install plugin requirements.\n"
        "The wheel under native/ is retained as a manual-install and diagnostic artifact.\n"
    ).encode()

    with zipfile.ZipFile(archive, "w", strict_timestamps=True) as bundle:
        for relative, source in source_files():
            bundle.writestr(archive_info(relative), source.read_bytes())
        for relative, payload in bundled_native_files(wheel, profile):
            bundle.writestr(archive_info(relative), payload)
        bundle.writestr(archive_info(PurePosixPath("INSTALL.txt")), install_note)
        bundle.writestr(
            archive_info(PurePosixPath("native") / wheel.name),
            wheel.read_bytes(),
        )

    shutil.copyfile(wheel, wheel_copy)
    checksums = output / "SHA256SUMS"
    checksums.write_text(
        "".join(
            f"{digest(path)}  {path.name}\n"
            for path in sorted((archive, wheel_copy), key=lambda item: item.name)
        ),
        encoding="utf-8",
        newline="\n",
    )
    print(archive)
    print(wheel_copy)
    print(checksums)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
