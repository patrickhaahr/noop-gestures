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
- `just dev run` runs one command per gesture, listed in `command` in `src/daemon.rs`: twist scrolls the focused app down 5 notches with `ydotool`; reverse twist calls `niri msg action toggle-overview`; left, right, workspace up and workspace down call the `focus-column-*` and `focus-workspace-*` action of the same name; knock runs `openhome ir edifier mute`. Swing labels are named after their action, so the takes were relabelled when bindings moved (backups sit next to `takes.jsonl`). `ydotool` talks to the `ydotoold` user service, whose socket is `$XDG_RUNTIME_DIR/.ydotool_socket` rather than the client's `/tmp` default, so the daemon sets `YDOTOOL_SOCKET` when it is unset; without it `ydotool` exits 0 and does nothing. A 1 s cooldown stops a return swing from firing the opposite gesture. Latency is 0.5 to 1 s, because the strap sends one batch per second.
- 1932-byte type-43 frames carry the optical waveform: 100 signed 24-bit little-endian samples per frame from frame offset 42 with a stride of 4, so the same 100 Hz as the IMU. The bytes past sample 100 are not waveform. The channel's wavelength is unknown. Takes store it as an optional `optical` field, and the recognizer does not use it yet.
- Finger gestures were tried and dropped. A fist produces almost no wrist motion (peak activity mostly under 60), and its downward optical drift showed only about half the time. A thumb-to-index pinch, interleaved with idle takes, had an optical signal that matched idle; the first all-pinch run had separated only because of something in that session.
- `just dev record-idle --minutes 8` records one continuous idle take of normal work. `evaluate` replays every idle take of 60 s or more through the live detector, trained without that take, and prints how many gestures would have fired; that count is the false-trigger rate to drive down.
