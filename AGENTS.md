# AGENTS

## Verification
- Run `just check` before handing back; it applies formatter and lint autofixes first, and the pre-commit hook runs it too. Stage everything it rewrote before committing.
- `just test-one <name>` runs one test while iterating.

## WHOOP 4.0 strap
- Protocol reference: noop's `docs/PROTOCOL_WHOOP4.md` and `docs/PROTOCOL_SENSORS.md` (repo `ryanbr/noop`).
- The phone's Bluetooth must be off: the strap serves one central at a time.
- Bond the strap the way it asks, not with `bluetoothctl pair`. A manual pair leaves command writes unanswered, and BlueZ then rejects every later write with `InProgress`. Instead: `bluetoothctl remove <mac>`, then in `bluetoothctl` run `agent NoInputNoOutput`, `default-agent`, `connect <mac>`, subscribe to `61080003`/`4`/`5` with `gatt.notify on`, and answer `yes` to the strap's "Accept pairing" request.
- `just dev calibrate` serves the calibrator on http://127.0.0.1:7878 and appends labelled takes to `$XDG_DATA_HOME/noop-gestures/takes.jsonl`; it only answers loopback Host headers and same-origin writes.
- `just dev probe [seconds] [--imu-stream]` runs the live IMU probe: command 63 enables type-43 frames, 1928-byte ones carry 100 accel + 100 gyro samples, delivered about once per second.
- The u32 at frame offset 15 of type-43 frames is not Unix time on firmware seen so far; use arrival time.
