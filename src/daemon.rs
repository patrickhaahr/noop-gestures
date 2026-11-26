use anyhow::{Result, ensure};
use tokio::sync::watch;

use crate::dataset::{Dataset, Label};
use crate::detector::{Detection, Detector};
use crate::recognizer::Recognizer;
use crate::session::{self, Event, log};
use crate::timeline::{TimedSample, Timeline};

const fn niri_action(label: Label) -> Option<&'static str> {
    match label {
        Label::Overview => Some("toggle-overview"),
        Label::Left => Some("focus-column-left"),
        Label::Right => Some("focus-column-right"),
        Label::WorkspaceUp => Some("focus-workspace-up"),
        Label::WorkspaceDown => Some("focus-workspace-down"),
        Label::ReverseTwist => Some("maximize-column"),
        Label::Knock => Some("center-column"),
        Label::Idle | Label::Fist | Label::Pinch => None,
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
    let Some(action) = niri_action(detection.label) else {
        return;
    };
    log(&format!(
        "{:?} -> {action} ({} ms after the peak)",
        detection.label,
        session::now_ms().saturating_sub(detection.t)
    ));
    tokio::spawn(async move {
        match tokio::process::Command::new("niri")
            .args(["msg", "action", action])
            .status()
            .await
        {
            Ok(status) if status.success() => {}
            Ok(status) => log(&format!("niri msg action {action} exited with {status}")),
            Err(e) => log(&format!("could not run niri: {e}")),
        }
    });
}
