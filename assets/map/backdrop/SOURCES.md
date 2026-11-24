# World backdrop sources

Fetched 2026-11-24. The 630 NOAA record files and their SHA-256 digests are listed in
`sources/records.sha256`; the large downloads belong in a cache outside working copies. The initial list was made by hashing each complete downloaded file; `fetch.py` checks subsequent downloads against it.

- Natural Earth 1:50m land, version 5.1.2: <https://github.com/nvkelso/natural-earth-vector/blob/v5.1.2/geojson/ne_50m_land.geojson>.
  Public domain; made with Natural Earth, free vector and raster map data at naturalearthdata.com.
- NOAA-CIRES-DOE Twentieth Century Reanalysis version 3: daily sea-level pressure and monthly 2 m air temperature,
  1836–1900, from <https://downloads.psl.noaa.gov/Datasets/20thC_ReanV3/>. The PSL files say the data are available free of charge under the PSD data policy; NCAR lists CC BY 4.0;
  dataset DOI <https://doi.org/10.5065/H93G-WS83>. 20th Century Reanalysis V3 data provided by NOAA PSL,
  Boulder, Colorado, from <https://psl.noaa.gov/>. Slivinski et al. (2019),
  <https://doi.org/10.1002/qj.3598>.
- NOAA ERSST version 6, monthly sea surface temperature 1854–1900, from
  <https://www.ncei.noaa.gov/data/sea-surface-temperature-extended-reconstructed/v6/access/>.
  The NCEI files state: "No constraints on data access or use". Their requested acknowledgement is:
  "The NOAA Extended Reconstructed Sea Surface Temperature (ERSST) data are provided by the NOAA National Centers for Environmental Information(NCEI)."
  Huang et al. (2025), dataset <https://doi.org/10.25921/z7cw-es89>; papers
  <https://doi.org/10.1175/JCLI-D-23-0707.1> and <https://doi.org/10.1175/JCLI-D-24-0185.1>.

The normals are historical reanalysis and reconstructed estimates, not observations at every backdrop block.
The backdrop grid is finer than the measurements, interpolates between them, and does not yet affect the game.

Use Python with NumPy 2.3.4 and Pillow 12.2.0 (the versions in `tools/requirements.txt`). The optional netCDF reader
is netCDF4 1.7.2, required only for reduction and kept outside the project environment. For example, from the
repository root, replacing the cache path with an external directory:

```sh
python3 -m venv /path/outside/copies/ck412-venv
/path/outside/copies/ck412-venv/bin/pip install netCDF4==1.7.2 numpy==2.3.4 Pillow==12.2.0
/path/outside/copies/ck412-venv/bin/python assets/map/backdrop/fetch.py /path/outside/copies/records assets/map/backdrop/sources/records.sha256
/path/outside/copies/ck412-venv/bin/python assets/map/backdrop/reduction.py /path/outside/copies/records assets/map/backdrop/sources
/path/outside/copies/ck412-venv/bin/python assets/map/backdrop/build.py /tmp/backdrop-output
```

The build reads only the committed reduced grids, outlines, and `settings.toml`; it does not use netCDF4 or network access.
To compare the map and scenario, run:

```sh
/path/outside/copies/ck412-venv/bin/python assets/map/backdrop/report.py assets/map/backdrop assets/map/ireland assets/scenarios/ireland-768/locations.toml assets/map/backdrop/points.toml /tmp/backdrop-report
```

The generated output files match the committed backdrop. SHA-256:

```text
ab7d71e60209eb88b7d7b5385c4f470e09984e8bfd5a45348b35ff0a5b360f66  assets/map/backdrop/settings.toml
cf6ebd2c9b45d2293b9823de9584992f2d528bd7d80628fc86b01339324f24f3  assets/map/backdrop/backdrop.toml
b677fbd7c714a25f0bf78b88477bb3ac8a870a1e31a2b9683c63a32957433d9f  assets/map/backdrop/air_temperature.png
71248a5c4458d28d0e92682a0fef42750473d822df58a425f25edbffe7decb8a  assets/map/backdrop/land.png
d4755535ce2c2c6f6c2a1cf0be6e5c2546d4eca66cf0a94ba6628fcae8f0678e  assets/map/backdrop/pressure.png
e47d48628fe6e94c92db877d2374b2d7579853faa36468ab463aea8f6a6d8002  assets/map/backdrop/sea_temperature.png
b3712000ad5797c116f93c0ffd3b44a5ec586e61fac56043226ec7506b13dfdf  assets/map/backdrop/sources/air_mean.png
e874b27a51d146452be360cafb3cc50c86001074a67d534113e6534682f9826b  assets/map/backdrop/sources/ne_50m_land.geojson
81e889272b6f58b6be4d9f21153815f629d71b3a6a0089b0980f976f5aff7dd4  assets/map/backdrop/sources/pressure_median.png
f6f63437cd1cfe6d97b834a828ac1129a0cf03021274aab0f7aea417fbd8eb84  assets/map/backdrop/sources/records.sha256
812c0755605cffd125cc3b1fdedf1b7862da24c309166d0165abc2fe3dc57cda  assets/map/backdrop/sources/sea_mean.png
```

Tool and fixture SHA-256:

```text
1f0e68ab85214d96c2f11c0f99025de277e0894d065b0bbd97209122ac89394e  assets/map/backdrop/geometry.py
8f3347848295d97b46bbb7996946f8e216f82cb8ea1adb629b8ed79d360aade8  assets/map/backdrop/fetch.py
94adbc82e946a2dc3b5e9f63409559c825537990e82e88cdfd5a690aee8c3af3  assets/map/backdrop/reduction.py
8a595b924982f17e2a146293b628fa83f0ffcadb13a93d662fee6cca1b91c8af  assets/map/backdrop/build.py
35258c21bac48df045fa99fb889491fbf67ed9709b5be80e5e191b1df92c2026  assets/map/backdrop/report.py
90b9faf8a5392dc68cb3ded84a73febc4d0cc7507bc5dfced6f2f4ee1d94e06f  assets/map/backdrop/points.toml
8ba848fb3e0e283e10e907b31c0afc94fc059038f21217bf350cf6735474b47e  crates/map/tests/fixtures/backdrop/air_temperature.png
0e3a37f3c68385cddcfc64248cd47f06931446d2d227ccb0b39f027c02b3ebec  crates/map/tests/fixtures/backdrop/backdrop.toml
7622b4e193f3d05fb48f70af1730361875c6e5225964831f526e8540944b4f0e  crates/map/tests/fixtures/backdrop/land.png
319c4b10f856ae0b8317f4ba4ab0224bb17d49ba137e51d726f98742266aea89  crates/map/tests/fixtures/backdrop/pressure.png
f4663a09484a71bef044bf4332d8a425634ed181fda2c641a68e6ed44f3778e9  crates/map/tests/fixtures/backdrop/sea_temperature.png
fa99fac53874fff9cc5fc2e44280874eec5351f6859b053eb16924aa325afaa3  crates/map/tests/fixtures/backdrop8192/air_temperature.png
9b17cbdabcc18dd0ec817017beb45440e82b4e99f8e4d130ff61c3529653d71f  crates/map/tests/fixtures/backdrop8192/backdrop.toml
2c42de07362483593a67e1dd422892a90a2ffb6f5b3979a5d1ee84fe1d8ff4d1  crates/map/tests/fixtures/backdrop8192/land.png
0a949adb981cfa45a1412fa0bbc25ac2f819bf29ac9fceaf27283a7b5f8a6280  crates/map/tests/fixtures/backdrop8192/pressure.png
ed02ac176b78918eac3b06c7cbab72dea144cce2dee4913afed4d70623939304  crates/map/tests/fixtures/backdrop8192/sea_temperature.png
```
