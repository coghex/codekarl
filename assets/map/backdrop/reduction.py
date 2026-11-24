"""Reduce the records to monthly normals on their own grids."""

import calendar
import tomllib
from collections.abc import Sequence
from datetime import date
from pathlib import Path

import numpy as np
from fetch import verify

SETTINGS = tomllib.loads((Path(__file__).parent / "settings.toml").read_text())
AIR_START, AIR_END = SETTINGS["air_first_year"], SETTINGS["air_last_year"]
PRESSURE_START, PRESSURE_END = SETTINGS["pressure_first_year"], SETTINGS["pressure_last_year"]
MONTHS, U16_MAX = 12, 65535
SEA_START, SEA_END = SETTINGS["sea_first_year"], SETTINGS["sea_last_year"]


def half_away(value: np.ndarray) -> np.ndarray:
    return np.copysign(np.floor(np.abs(value) + 0.5), value).astype(np.int64)


def order_axes(values: np.ndarray, lat: np.ndarray, lon: np.ndarray) -> np.ndarray:
    return np.take(np.take(values, np.argsort(-lat), axis=-2), np.argsort(lon % 360), axis=-1)


def sea_normals(values: np.ndarray) -> np.ndarray:
    values = np.asarray(values, dtype=np.float64)
    if values.shape[:2] != (SEA_END - SEA_START + 1, 12):
        raise ValueError("sea record must contain the configured years and twelve months")
    valid = np.isfinite(values).all(axis=(0, 1))
    if not valid.any():
        raise ValueError("sea_mean.png: month 1 has no sea value")
    means = np.where(np.isfinite(values), values, 0).mean(axis=0)
    result = half_away(means * 100) + 10000
    result[:, ~valid] = 0
    return result


def read_sea_records(cache: Path, expected: dict[str, str]) -> np.ndarray:
    """Read each dated monthly file after checking its pinned digest and axes."""
    from netCDF4 import (  # noqa: PLC0415  # optional reader is needed only for reduction
        Dataset,
        num2date,
    )

    values = []
    for year in range(SEA_START, SEA_END + 1):
        for month in range(1, 13):
            name = f"ersst.v6.{year}{month:02}.nc"
            path = cache / name
            verify(path, expected[name])
            with Dataset(path) as data:
                time = data["time"]
                dates = num2date(
                    time[:], time.units, calendar=getattr(time, "calendar", "standard")
                )
                if len(dates) != 1 or (dates[0].year, dates[0].month) != (year, month):
                    raise ValueError(f"{name}: unexpected date")
                lat = np.asarray(data["lat"][:])
                lon = np.asarray(data["lon"][:])
                if not np.array_equal(np.sort(lat), np.arange(-88, 89, 2)) or not np.array_equal(
                    np.sort(lon % 360), np.arange(0, 360, 2)
                ):
                    raise ValueError(f"{name}: unexpected coordinate axes")
                var = data["sst"]
                dims = list(var.dimensions)
                if (
                    set(dims) != {"time", "lev", "lat", "lon"}
                    or var.shape[dims.index("time")] != 1
                    or var.shape[dims.index("lev")] != 1
                ):
                    raise ValueError(f"{name}: unexpected dimensions")
                array = np.ma.asarray(var[:]).astype(np.float64).filled(np.nan)
                array = np.transpose(array, [dims.index(k) for k in ("time", "lev", "lat", "lon")])[
                    0, 0
                ]
                values.append(order_axes(array, lat, lon))
    return np.asarray(values).reshape(SEA_END - SEA_START + 1, 12, 89, 180)


def pressure_median(values: np.ndarray) -> np.ndarray:
    values = np.asarray(values, dtype=np.float64)
    if not np.isfinite(values).all():
        raise ValueError("pressure contains a missing value")
    return half_away(np.median(values, axis=0))


def air_mean(values: np.ndarray) -> np.ndarray:
    values = np.asarray(values, dtype=np.float64)
    if not np.isfinite(values).all():
        raise ValueError("air contains a missing value")
    return half_away((np.mean(values, axis=0) - 273.15) * 100)


def checked_axes(data: np.ndarray, name: str, spacing: int) -> tuple[np.ndarray, np.ndarray]:
    lat = np.asarray(data["lat"][:], dtype=np.float64)
    lon = np.asarray(data["lon"][:], dtype=np.float64)
    top = 90 if spacing == 1 else 88
    if not np.array_equal(np.sort(lat), np.arange(-top, top + 1, spacing)):
        raise ValueError(f"{name}: unexpected latitude axis")
    if not np.array_equal(np.sort(lon % 360), np.arange(0, 360, spacing)):
        raise ValueError(f"{name}: unexpected longitude axis")
    return lat, lon


def reduce_air(path: Path, expected: str) -> np.ndarray:
    from netCDF4 import (  # noqa: PLC0415  # optional reader is needed only for reduction
        Dataset,
        num2date,
    )

    name = path.name
    verify(path, expected)
    with Dataset(path) as data:
        lat, lon = checked_axes(data, name, 1)
        t = data["time"]
        dates = num2date(t[:], t.units, calendar=getattr(t, "calendar", "standard"))
        selected = [(i, d) for i, d in enumerate(dates) if AIR_START <= d.year <= AIR_END]
        index = {(d.year, d.month): i for i, d in selected}
        if (
            len(selected) != (AIR_END - AIR_START + 1) * 12
            or len(index) != (AIR_END - AIR_START + 1) * 12
        ):
            raise ValueError(f"{name}: missing or duplicate months")
        var = data["air"]
        dims = list(var.dimensions)
        if set(dims) != {"time", "lat", "lon"}:
            raise ValueError(f"{name}: unexpected dimensions")
        result = []
        for month in range(1, 13):
            selection = [slice(None)] * 3
            selection[dims.index("time")] = [
                index[(year, month)] for year in range(AIR_START, AIR_END + 1)
            ]
            samples = np.ma.asarray(var[tuple(selection)]).astype(np.float64).filled(np.nan)
            samples = np.transpose(samples, [dims.index(k) for k in ("time", "lat", "lon")])
            result.append(air_mean(order_axes(samples, lat, lon)))
    return np.asarray(result)


def month_indices(dates: Sequence[date], year: int, month: int, name: str) -> list[int]:
    indices = sorted(
        (i for i, d in enumerate(dates) if d.year == year and d.month == month),
        key=lambda i: dates[i].day,
    )
    days = [dates[i].day for i in indices]
    if days != list(range(1, calendar.monthrange(year, month)[1] + 1)):
        raise ValueError(f"{name}: missing or duplicate days in month {month}")
    return indices


def reduce_pressure(cache: Path, expected: dict[str, str]) -> np.ndarray:
    from netCDF4 import (  # noqa: PLC0415  # optional reader is needed only for reduction
        Dataset,
        num2date,
    )

    for year in range(PRESSURE_START, PRESSURE_END + 1):
        name = f"prmsl.{year}.nc"
        verify(cache / name, expected[name])
    result = []
    for month in range(1, 13):
        samples = []
        for year in range(PRESSURE_START, PRESSURE_END + 1):
            name = f"prmsl.{year}.nc"
            with Dataset(cache / name) as data:
                lat, lon = checked_axes(data, name, 1)
                t = data["time"]
                dates = num2date(t[:], t.units, calendar=getattr(t, "calendar", "standard"))
                indices = month_indices(dates, year, month, name)
                var = data["prmsl"]
                dims = list(var.dimensions)
                if set(dims) != {"time", "lat", "lon"}:
                    raise ValueError(f"{name}: unexpected dimensions")
                select = [slice(None)] * 3
                select[dims.index("time")] = indices
                a = np.ma.asarray(var[tuple(select)]).astype(np.float64).filled(np.nan)
                a = np.transpose(a, [dims.index(k) for k in ("time", "lat", "lon")])
                a = order_axes(a, lat, lon)
                if not np.isfinite(a).all():
                    raise ValueError(f"{name}: missing pressure in month {month}")
                samples.append(a)
        result.append(pressure_median(np.concatenate(samples, axis=0)))
    return np.asarray(result)


def write_grid(path: Path, values: np.ndarray) -> None:
    """Write twelve stacked, lossless 16-bit rows of source-grid data."""
    from PIL import Image  # noqa: PLC0415  # image writing is only used by the command

    if values.shape[0] != MONTHS or np.any(values < 0) or np.any(values > U16_MAX):
        raise ValueError(f"{path.name}: values outside 16-bit storage")
    image = values.astype(np.uint16).reshape(values.shape[0] * values.shape[1], values.shape[2])
    Image.fromarray(image).save(path, format="PNG", optimize=True)


def main() -> None:
    import argparse  # noqa: PLC0415  # only the command needs argument parsing

    parser = argparse.ArgumentParser(description="Reduce pinned records with netCDF4==1.7.2")
    parser.add_argument("cache", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    digest_file = Path(__file__).parent / "sources/records.sha256"
    expected = {
        name: digest
        for digest, name in (line.split() for line in digest_file.read_text().splitlines())
    }
    sea = sea_normals(read_sea_records(args.cache, expected))
    air_name = "air.2m.mon.mean.nc"
    air = reduce_air(args.cache / air_name, expected[air_name]) + 10000
    pressure = reduce_pressure(args.cache, expected) - 60000
    args.output.mkdir(parents=True, exist_ok=True)
    write_grid(args.output / "sea_mean.png", sea)
    write_grid(args.output / "air_mean.png", air)
    write_grid(args.output / "pressure_median.png", pressure)


if __name__ == "__main__":
    main()
