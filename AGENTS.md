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

## Recognizer and daemon
- Only one process may hold the strap: stop the calibrator before `just dev run`, or the daemon connects but receives no IMU data.
- `just dev evaluate` holds out one take at a time and prints the confusion matrix; run it after recording takes, before trusting `run`.
- The recognizer is 1-nearest-neighbour over 1.2 s windows (gyro/200 dps and raw accel) centred on activity peaks, where activity is gyro magnitude plus 50 dps per g that the acceleration magnitude strays from 1 g, so a knock with no rotation still registers; idle takes become negative templates at every peak above 60. Re-sweep that weight with `evaluate` after recording knock takes. A distance-margin reject rule and DTW were tried offline and lost accuracy.
- `just dev run` maps overview, left, right, workspace up, workspace down, reverse twist and knock to `niri msg action toggle-overview`, `focus-column-left`, `focus-column-right`, `focus-workspace-up` and `focus-workspace-down`, with reverse twist on `maximize-column` and knock on `center-column`, with a 1 s cooldown so a return swing does not fire the opposite gesture. Latency is 0.5 to 1 s, because the strap sends one batch per second.
