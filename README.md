# noop-gestures

Turn WHOOP 4.0 wrist motion over BLE into desktop actions for [niri](https://github.com/YaLTeR/niri) and Wayland (`ydotool`, `playerctl`, etc.).

## Gestures

| Gesture | Action |
|---|---|
| `twist` | Scroll down 5 notches (`ydotool`) |
| `reverse_twist` | `niri` `ToggleOverview` |
| `left` / `right` | `niri` `FocusColumnLeft` / `FocusColumnRight` |
| `up` / `down` | `niri` `FocusWorkspaceUp` / `FocusWorkspaceDown` |
| `knock` | Play/pause Spotify (`playerctl -p spotify play-pause`) |

Bindings live in [`action`](src/daemon.rs).

## Setup

### 1. Bond the WHOOP 4.0 strap (Linux / BlueZ)

Turn off Bluetooth on your phone first — the strap only serves one central at a time. Bond the way the strap initiates it (do **not** use `bluetoothctl pair`):

```bash
bluetoothctl remove <mac>
bluetoothctl
> agent NoInputNoOutput
> default-agent
> connect <mac>
> menu gatt
> select-attribute 61080003-8d6d-82b8-614a-1c8cb0f8dcc6
> notify on
# Repeat `notify on` for 61080004-... and 61080005-...
# Answer `yes` when the strap prompts "Accept pairing"
```

### 2. Record takes and run

Takes are stored in `$XDG_DATA_HOME/noop-gestures/takes.jsonl` (default `~/.local/share/noop-gestures/takes.jsonl`). Only one process can hold the strap at a time.

```bash
# 1. Record labelled gesture takes via local web UI (http://127.0.0.1:7878)
just dev calibrate

# 2. Record normal desk work so the recognizer learns negative (idle) peaks
just dev record-idle --minutes 8

# 3. Check held-out accuracy, long-idle false triggers, and batch latency
cargo build --release
./target/release/noop-gestures evaluate
./target/release/noop-gestures benchmark

# 4. Run the live gesture daemon
./target/release/noop-gestures run
```

For scrolling via `ydotool`, make sure `ydotoold` is running (`noop-gestures` defaults `YDOTOOL_SOCKET` to `$XDG_RUNTIME_DIR/.ydotool_socket` if unset).

## How it works

The WHOOP 4.0 streams 100-sample 6-axis IMU frames (~104 Hz, one 1928-byte batch every ~960 ms) over BLE notifications. `Timeline` fits the sample period to earliest arrivals so no samples are dropped, and `Detector` classifies in three tiers over `[gyro/200 dps, accel_g]`:

1. **Full window** (`[-25, +22]` around a peak) once 22 post-peak samples arrive.
2. **Early peak prefix** (`1..21` samples past a peak at batch end) using top-3 per-label nearest-template mean distance.
3. **Rising edge** (at sample 99, up to 25 samples *before* the peak arrives in the next batch) comparing active rising tails against distinct gesture takes and active idle tails, with a 100-sample cooldown to prevent double-fires.

See [docs/latency.md](docs/latency.md) for full latency benchmarks, protocol notes, what worked, and what didn't.

## Extra performance

By default on Linux/BlueZ, the WHOOP 4.0 negotiates a **60 ms** BLE connection interval. That adds ~30 ms average wait before a batch starts transmitting and causes ~35–40% of 8-fragment IMU frames to spill across 2–4 connection events (**+120 to +363 ms** stalls).

### Initial interval (NixOS / BlueZ)

You can lower BlueZ's default LE connection interval in your NixOS config:

```nix
hardware.bluetooth.settings.LE = {
  MinConnectionInterval = 6;   # 7.5 ms (units of 1.25 ms)
  MaxConnectionInterval = 12;  # 15 ms
  ConnectionLatency = 0;
};
```

Or temporarily until reboot via `btmgmt`:

```bash
doas btmgmt set-sysconfig -v 0017:2:0600 0018:2:0c00 0019:2:0000
```

### Keeping it at 7.5 ms after connect

The catch: ~0.9 s after connecting, the WHOOP 4.0 strap sends its own L2CAP Connection Parameter Update Request advertising `[7.5 ms .. 75 ms]`, and Linux's Bluetooth stack picks **60 ms** from that range (and caches `MaxInterval=60` in `/var/lib/bluetooth/<adapter>/<mac>/info`).

To lock the live link at the BLE minimum (**7.50 ms**, which the strap accepts and holds), run `hcitool lecup` ~1–2 s after the daemon logs `strap Streaming`:

```bash
HANDLE=$(doas hcitool con | awk '/<STRAP_MAC>/ {for (i=1; i<=NF; i++) if ($i=="handle") print $(i+1)}')
doas hcitool lecup --handle "$HANDLE" --min 6 --max 8 --latency 0 --timeout 400
```

Verify with `doas btmon` (`LE Connection Update Complete` -> `Connection interval: 7.50 msec (0x0006)`). Tighter intervals eliminate the 120–363 ms BLE fragment-spread stalls at a modest strap battery cost.
