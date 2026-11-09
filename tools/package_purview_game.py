#!/usr/bin/env python3
"""Package the Windows game and its original preparation files in one executable."""
import argparse
from pathlib import Path
import shutil
import subprocess
from zipfile import ZIP_DEFLATED, ZipFile

ROOT = Path(__file__).resolve().parents[1]
SOURCES = (
    "research-lock.json",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "crates/horizon-prepare",
    "mods/horizon/src/test/java",
    "tools",
)
BINARIES = ("horizon-client.exe", "horizon-ctl.exe", "horizon-prepare.exe")


def package(binaries, output):
    for name in BINARIES:
        if not (binaries / name).is_file():
            raise FileNotFoundError(binaries / name)
    engine = binaries / BINARIES[0]
    if output.resolve() == engine.resolve():
        raise ValueError("The bundle must be a different file from the original engine.")
    files = subprocess.check_output(
        ["git", "ls-files", "-z", "--", *SOURCES], cwd=ROOT
    ).decode().split("\0")
    output.parent.mkdir(parents=True, exist_ok=True)
    partial = output.with_suffix(".partial")
    shutil.copyfile(engine, partial)
    try:
        with ZipFile(partial, "a", compression=ZIP_DEFLATED) as bundle:
            for name in BINARIES:
                bundle.write(binaries / name, name)
            for name in files:
                if name and (ROOT / name).is_file():
                    bundle.write(ROOT / name, name)
        partial.replace(output)
    finally:
        partial.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    package(args.bin_dir, args.out)
