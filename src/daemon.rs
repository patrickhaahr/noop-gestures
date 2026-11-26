use anyhow::{Result, ensure};
use tokio::sync::watch;

use crate::dataset::{Dataset, Label};
use crate::detector::{Detection, Detector};
use crate::recognizer::Recognizer;
use crate::session::{self, Event, log};
use crate::timeline::{TimedSample, Timeline};

const fn command(label: Label) -> Option<&'static [&'static str]> {
    match label {
        Label::Overview => Some(&["niri", "msg", "action", "toggle-overview"]),
        Label::Left => Some(&["niri", "msg", "action", "focus-column-left"]),
        Label::Right => Some(&["niri", "msg", "action", "focus-column-right"]),
        Label::WorkspaceUp => Some(&["niri", "msg", "action", "focus-workspace-up"]),
        Label::WorkspaceDown => Some(&["niri", "msg", "action", "focus-workspace-down"]),
        Label::ReverseTwist => Some(&["ydotool", "mousemove", "--wheel", "--", "0", "-5"]),
        Label::Knock => Some(&["openhome", "ir", "edifier", "mute"]),
        Label::Fist => Some(&["playerctl", "play-pause"]),
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

    let emit = move |event| match event {
        Event::Status(status) => log(&format!("strap {status:?}")),
        Event::Batch {
            samples,
            arrived_ms,
        } => {
            let stamped = timeline.push(&samples, arrived_ms);
            for detection in detector.push(&stamped) {
                fire(detection);
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

fn fire(detection: Detection) {
    let Some(argv @ &[program, ref args @ ..]) = command(detection.label) else {
        return;
    };
    let shown = argv.join(" ");
    log(&format!(
        "{:?} -> {shown} ({} ms after the peak)",
        detection.label,
        session::now_ms().saturating_sub(detection.t)
    ));
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
        match process.status().await {
            Ok(status) if status.success() => {}
            Ok(status) => log(&format!("{shown} exited with {status}")),
            Err(e) => log(&format!("could not run {shown}: {e}")),
        }
    });
}
