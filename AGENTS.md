# AGENTS

## Verification
- Run `just check` before handing back; it runs format, clippy autofixes, clippy `-D warnings`, `comment-guard`, and tests (the pre-commit hook runs it too). Stage any files it rewrites before committing.
- Use `just test-one <name>` while iterating on a single test.
- Run `./target/release/noop-gestures evaluate` and `./target/release/noop-gestures benchmark` after changing takes or recognizer logic before trusting `run`; treat long-idle false triggers as the primary regression bar alongside latency.

## Pointers
- **Latency, recognizer experiments, BLE limits, or finger gestures:** read [`docs/latency.md`](docs/latency.md) before tuning thresholds, trying new classifiers/features, probing firmware streams, or revisiting optical/finger gestures.
- **WHOOP 4.0 wire protocol:** consult `ryanbr/noop`'s `docs/PROTOCOL_WHOOP4.md` and `docs/PROTOCOL_SENSORS.md` before adding commands or frame decoders.
- **Setup, bonding, and BLE interval tuning:** see [`README.md`](README.md).

## Hardware & BLE gotchas
- **Single central & single process:** turn the phone's Bluetooth off and stop `calibrate` before `run` or `probe`; a second client connects silently with zero IMU data.
- **Bonding:** always bond via strap-initiated pairing (see [`README.md`](README.md)); `bluetoothctl pair` leaves command writes unanswered and stuck in `InProgress`.
- **Worn vs. unworn:** an unworn strap replies `pending` to command 63 and emits no type-43 frames; run live stream probes while wearing the strap.
- **Unreliable strap clock:** the `u32` at offset 15 of type-43 frames is not Unix time on this firmware; stamp from host arrival time instead.
- **Optical frames:** 1932-byte type-43 frames carry 100 samples starting at offset 42 (stride 4); bytes past sample 100 are not waveform, the wavelength is unknown, and the recognizer currently ignores `optical`.

## Recognizer & daemon invariants
- **~104 Hz timeline:** the strap samples at ~104 Hz (~960 ms per 100-sample batch). `Timeline` fits the sample period to earliest arrivals so late batches shift no timestamps and drop no samples; `Detector` keys on sample count and cooldown samples, using timestamps only for reporting.
- **Label naming:** swing labels match their desktop action (see `action` in `src/daemon.rs`), not physical wrist direction; backups of relabelled datasets sit next to `$XDG_DATA_HOME/noop-gestures/takes.jsonl`.
- **`ydotool` socket:** `ydotoold` listens at `$XDG_RUNTIME_DIR/.ydotool_socket`, whereas `ydotool` defaults to `/tmp` and exits `0` doing nothing when unset; keep the fallback env wiring in `src/daemon.rs`.
