use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use tokio::sync::watch;

use crate::dataset::{Dataset, Label};
use crate::detector::{Detection, Detector, Evidence};
use crate::niri::Niri;
use crate::recognizer::{POST_SAMPLES, Recognizer};
use crate::session::{self, Event, log};
use crate::timeline::{TimedSample, Timeline};

enum Action {
    Process(&'static str, &'static [&'static str]),
    Niri(&'static str),
}

const fn action(label: Label) -> Option<Action> {
    match label {
        Label::Twist => Some(Action::Process(
            "ydotool",
            &["mousemove", "--wheel", "--", "0", "-5"],
        )),
        Label::ReverseTwist => Some(Action::Niri("ToggleOverview")),
        Label::Left => Some(Action::Niri("FocusColumnLeft")),
        Label::Right => Some(Action::Niri("FocusColumnRight")),
        Label::Up => Some(Action::Niri("FocusWorkspaceUp")),
        Label::Down => Some(Action::Niri("FocusWorkspaceDown")),
        Label::Knock => Some(Action::Process("openhome", &["ir", "edifier", "mute"])),
        Label::Idle => None,
    }
}

pub async fn run(dataset: &Dataset) -> Result<()> {
    let counts = dataset.counts()?;
    ensure!(
        counts.get(&Label::Idle).is_some_and(|&n| n > 0),
        "record idle takes with `noop-gestures calibrate` first ({})",
        dataset.path().display()
    );
    let mut detector = Detector::new(Recognizer::train(&dataset.takes()?));
    let mut timeline = Timeline::<TimedSample>::default();
    log(&format!(
        "trained on {counts:?} from {}",
        dataset.path().display()
    ));

    let niri = Niri::connect_lazily();
    let emit = move |event| match event {
        Event::Status(status) => log(&format!("strap {status:?}")),
        Event::Batch {
            samples,
            arrived_ms,
        } => {
            let stamped = timeline.push(&samples, arrived_ms);
            let scan_started = Instant::now();
            let detections = detector.push(&stamped);
            let scan = scan_started.elapsed();
            for detection in detections {
                fire(detection, arrived_ms, scan, &niri);
            }
        }
        Event::Optical { .. } => {}
    };
    let (shutdown_tx, shutdown) = watch::channel(false);
    let session = tokio::spawn(session::run(emit, shutdown));
    session::shutdown_signal().await;
    shutdown_tx.send(true).ok();
    session.await?;
    Ok(())
}

fn fire(detection: Detection, arrived_ms: u64, scan: Duration, niri: &Niri) {
    let Some(action) = action(detection.label) else {
        return;
    };
    let shown = match action {
        Action::Process(program, args) => format!("{program} {}", args.join(" ")),
        Action::Niri(name) => format!("niri {name}"),
    };
    let evidence = match detection.evidence {
        Evidence::FullWindow => "on the full window".to_owned(),
        Evidence::Prefix { samples_past_peak } => {
            format!("early on {samples_past_peak} of {POST_SAMPLES} samples past the peak")
        }
    };
    log(&format!(
        "{:?} -> {shown}, {evidence}: its batch arrived {} ms after the peak, which was sample {} of its batch; {} later batches waited for; detector took {} us",
        detection.label,
        arrived_ms.saturating_sub(detection.t),
        detection.index_in_batch,
        detection.batches_waited,
        scan.as_micros()
    ));
    match action {
        Action::Niri(name) => niri.dispatch(name),
        Action::Process(program, args) => spawn(program, args, shown),
    }
}

fn spawn(program: &str, args: &[&str], shown: String) {
    let spawned = Instant::now();
    let mut process = tokio::process::Command::new(program);
    process.args(args);
    if std::env::var_os("YDOTOOL_SOCKET").is_none()
        && let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        process.env(
            "YDOTOOL_SOCKET",
            std::path::Path::new(&runtime).join(".ydotool_socket"),
        );
    }
    tokio::spawn(async move {
        let outcome = process.status().await;
        let ran_ms = spawned.elapsed().as_millis();
        match outcome {
            Ok(status) if status.success() => log(&format!("{shown} exited after {ran_ms} ms")),
            Ok(status) => log(&format!("{shown} exited with {status} after {ran_ms} ms")),
            Err(e) => log(&format!("could not run {shown}: {e}")),
        }
    });
}
