"""Fetch the pinned NOAA records into an external cache."""

import argparse
import hashlib
import time
import urllib.request
from pathlib import Path

RETRIES = 6
PRESSURE = "https://downloads.psl.noaa.gov/Datasets/20thC_ReanV3/Dailies/miscSI/"
AIR = "https://downloads.psl.noaa.gov/Datasets/20thC_ReanV3/Monthlies/2mSI-MO/"
SEA = "https://www.ncei.noaa.gov/data/sea-surface-temperature-extended-reconstructed/v6/access/"


def sha256(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def verify(path: Path, expected: str) -> None:
    if sha256(path) != expected:
        raise ValueError(f"{path.name}: SHA-256 mismatch")


def url_for(name: str) -> str:
    if name.startswith("prmsl."):
        return PRESSURE + name
    if name == "air.2m.mon.mean.nc":
        return AIR + name
    if name.startswith("ersst.v6."):
        return SEA + name
    raise ValueError(f"unknown source {name}")


def fetch(name: str, expected: str, cache: Path) -> None:
    target = cache / name
    if target.is_file():
        verify(target, expected)
        return
    part = cache / (name + ".part")
    for attempt in range(RETRIES):
        try:
            with (
                urllib.request.urlopen(url_for(name), timeout=120) as response,
                part.open("wb") as out,
            ):
                while block := response.read(1024 * 1024):
                    out.write(block)
            if sha256(part) != expected:
                raise ValueError(f"{name}: SHA-256 mismatch")
            part.replace(target)
            return
        except (OSError, ValueError):
            part.unlink(missing_ok=True)
            if attempt == RETRIES - 1:
                raise
            time.sleep(2**attempt)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Use an external venv with netCDF4==1.7.2 for reduction."
    )
    parser.add_argument("cache", type=Path)
    parser.add_argument("digests", type=Path)
    args = parser.parse_args()
    hub = Path(__file__).resolve().parents[3]
    if args.cache.resolve().is_relative_to(hub):
        raise ValueError(f"cache {args.cache} must be outside the working copy")
    args.cache.mkdir(parents=True, exist_ok=True)
    for line in args.digests.read_text().splitlines():
        digest, name = line.split()
        fetch(name, digest, args.cache)


if __name__ == "__main__":
    main()
