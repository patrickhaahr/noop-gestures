# Latency & Performance Findings

Notes from profiling and optimizing end-to-end wrist-gesture latency on a WHOOP 4.0 strap (firmware 41.17.6.0-class) over Linux BlueZ.

## The hardware constraint

The WHOOP 4.0 buffers **100 six-axis IMU samples** at **~104 Hz** (`~9.615 ms`/sample) and transmits one 1928-byte type-43 frame every **~960 ms** (8 ATT notifications at MTU 247), immediately followed by a 1932-byte optical frame.

If a detector only acts once a gesture's peak is fully enclosed in a batch, uniform placement inside a 960 ms batch puts the naive floor at `~480 ms + transport` — plus an extra **+960 ms** whenever the look-ahead window spills into the next batch.

## Progression

### Held-out offline benchmark (`noop-gestures benchmark`, 28 placements/take)

| Stage | Recognised | Wrong | Missed | Latency mean | Latency p50 | Latency p95 | Long-idle false fires (26 min) | Short-idle false fires |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Symmetric `[-60, +60]` window (`master` baseline)* | — | — | — | 1130 ms | — | — | 70 | — |
| Asymmetric `[-25, +22]` + 1-NN early (`post >= 3`, `r = 0.75`) | 5909 / 6300 | 56 | 335 | 601 ms | 601 ms | 1042 ms | 33 | 56 / 1120 |
| **+ Top-3 early (`post >= 1`) + rising-edge (`lead 0..25`) + full idle margin (`0.95`)** | 5848 / 6300 | 70 | 382 | **470 ms** | **460 ms** | **916 ms** | **28** | **3 / 1120** |

*\*Measured on the pre-fast-takes dataset before fast takes were recorded; the last two rows are on the identical 268-take dataset (225 gesture takes + 40 short idle + 3 long idle totaling 26 min).*

Note: `benchmark` measures from the detected sample (`sample 99` on rising-edge fires, reporting ~90 ms). Measured from the gesture's true peak sample, mean latency dropped from **547 ms → ~405 ms** (including 90 ms simulated transport, i.e., **~315 ms** of batch wait — 160 ms below the 475 ms half-batch barrier).

### Live runs on worn WHOOP 4.0

| Live run | Fires | Early post-peak | Rising-edge (pre-peak) | Waited 2nd batch | Mean | Median | p95 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Asymmetric `[-25, +22]`, 1-NN early (before fast takes, 5 min) | 48 | 3 | 0 | 9 (19%) | 714 ms | 725 ms | 1142 ms |
| + Fast gesture takes (5 min, 60 ms BLE) | 58 | 9 | 0 | 2 (3%) | 571 ms | 540 ms | 1048 ms |
| + Fast right takes (2 min, 60 ms BLE) | 35 | 6 | 0 | 7 (20%) | 715 ms | 833 ms | 1138 ms |
| **+ Top-3 early + rising-edge (2 min, 60 ms BLE)** | 21 | 3 | 2 | 2 (10%) | **523 ms** | 586 ms | 1104 ms |
| **+ Top-3 early + rising-edge (2 min, 7.5 ms BLE)** | 55 | 14 | 2 | **1 (1.8%)** | **503 ms** | **489 ms** | **971 ms** |

## What worked

- **Fixing the 104 Hz `Timeline` bug (`src/timeline.rs`):** Assuming 100 Hz (10 ms/sample) drifted 40 ms ahead per batch and dropped ~42 samples every ~10 batches on resync, while under-reporting latency by up to 400 ms. Fitting the period to earliest arrivals and keying `Detector` on sample count fixed both.
- **Persistent `niri` IPC connection (`src/niri.rs`):** Keeping one Unix socket open to `$NIRI_SOCKET` (with reconnect on compositor restart) dropped compositor dispatch to `0–1 ms`.
- **Asymmetric `[-25, +22]` window + early boundary firing:** Shrinking post-peak look-ahead from 60 to 22 samples and firing early near batch boundaries cut latency roughly in half vs `master`.
- **Recording fast gesture takes:** Sharper peaks hit earlier in a batch and were easier to separate on short prefixes.
- **Top-3 per-label early classification down to `EARLY_MIN_POST = 1`:** With 1-NN, a single stray idle peak among thousands of idle samples blocked early firing (forcing a +960 ms wait) on peaks even 20 samples past the peak. Averaging the top 3 nearest templates per label raised the safe margin from `0.75` to `0.94` and cut second-batch waits from ~20% down to **1.8%** live (0.7% in benchmark).
- **Pre-peak rising-edge detection at sample 99 (`Evidence::RisingEdge`):** Every gesture's rising edge is already distinctive 6–18 samples *before* its peak. Matching the 16-sample active rising tail at batch end against distinct gesture takes' pre-peak leads (`0..25`) and active rising idle tails fires on batch `N` instead of waiting ~960 ms for batch `N+1`, without double-firing.
- **Full-window idle margin (`FULL_IDLE_RATIO = 0.95`):** Cut short-idle false fires from 56 → 3 and long-idle false fires from 33 → 28 while keeping recall at 93%.
- **`Arc`-shared templates:** Made `holding_out` O(1) and sped up the full 6,300-replay benchmark ~8× (16 s → 2 s); live detector CPU time stays ~200 µs/batch.
- **Post-connect `hcitool lecup` to `7.50 ms`:** Overrode the strap's 60 ms negotiated BLE interval down to `7.50 ms` (`0x0006`), eliminating the 120–363 ms multi-event BLE fragment-spread stalls observed in live probes (`spread` stayed 8–14 ms).

## What didn't work

- **Protocol/firmware knobs for smaller batches:** Probed while worn with command 106 (`SET_IMU_DATA_STREAM`) and read-only gets 107, 130, 65, 117/118. Command 106 still only produces 100-sample ~960 ms type-43 frames; no smaller batch or streaming cadence is exposed on this firmware.
- **Early IMU decode at fragment 6 (byte 1292) before the unused tail + CRC32:** All 100 accel+gyro samples end at byte 1292 (fragment 6 of 8), leaving 636 bytes of unused tail + CRC32 in fragments 7–8. Live fragment-by-fragment timestamps showed multi-event stalls occurred on fragments 2–6, with only 2–3 ms between fragment 6 and 8 — not worth giving up CRC32 verification.
- **`btmgmt set-sysconfig` / `hardware.bluetooth.settings.LE` alone:** Sets the initial connection interval to 15 ms, but ~0.9 s after connect the WHOOP 4.0 sends an L2CAP Connection Parameter Update Request for `[7.5..75 ms]` and Linux chooses `60.00 ms` (and caches `MaxInterval=60` in `/var/lib/bluetooth/.../info`). Post-connect `hcitool lecup` is required to keep `7.50 ms`.
- **Eliminating all batch jitter via BLE:** Even locked at `7.50 ms` BLE, batch arrival jitter relative to the earliest-arrival line remained ~49 ms median / 140 ms p90 / 184 ms max, indicating internal batching/transfer jitter inside the WHOOP 4.0 between its sensor MCU and Nordic BLE chip.
- **Naive RBF kernel posterior over all templates:** Summing `exp(-d²/τ)` across 2,000+ idle templates vs 30 templates per gesture introduced a ~67× idle prior bias and missed 1,251 of 6,300 gesture replays.
- **Subtracting static gravity (`a - a[0]`):** Wrist orientation carries real gesture signal; removing it increased wrong gesture fires from 56 → 284 and short-idle false fires to 169.
- **DTW and full-window distance-margin reject rules:** Both lost offline accuracy compared to Euclidean nearest-template matching.
- **Unguarded pre-peak threshold (`mags[99] >= min_act` alone):** Fired repeatedly on flat high-activity segments and decaying tails until gated by `is_rising_tail` (minimum gain over recent 12-sample baseline + ≥ 70% retained of recent 6-sample peak).
- **Treating `EARLY_MIN_POST = 0` as a local peak:** `is_peak` at the final sample of a batch has an empty `after` slice, so any monotonically rising edge passed `is_peak` and got scored against peak-aligned templates instead of rising-edge templates. Keeping `EARLY_MIN_POST = 1` for `Prefix` and routing sample 99 to `RisingEdge` (`lead = 0..25`) fixed the ambiguity cleanly.
- **Finger gestures (fist & thumb-index pinch):** Fist motion stayed mostly under the 60 dps activity threshold; interleaved pinch optical signals matched idle.
