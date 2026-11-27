use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Result, ensure};
use tokio::sync::watch;

use crate::dataset::{Dataset, Label, Take};
use crate::session::{self, Event, log};
use crate::timeline::{TimedSample, Timeline};

#[derive(Default)]
struct Recording {
    clock: Timeline,
    samples: Vec<TimedSample>,
}

impl Recording {
    fn push(&mut self, samples: &[crate::imu::Sample], arrived_ms: u64) {
        let stamped = self.clock.push(samples, arrived_ms);
        if let Some(first) = stamped.first() {
            while self.samples.last().is_some_and(|s| s.t >= first.t) {
                self.samples.pop();
            }
        }
        self.samples.extend(stamped);
    }
}

fn lock(recording: &Mutex<Recording>) -> std::sync::MutexGuard<'_, Recording> {
    recording.lock().unwrap_or_else(PoisonError::into_inner)
}

pub async fn idle(dataset: &Dataset, duration: Duration) -> Result<()> {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let sink = Arc::clone(&recording);
    let emit = move |event| match event {
        Event::Status(status) => log(&format!("strap {status:?}")),
        Event::Batch {
            samples,
            arrived_ms,
        } => lock(&sink).push(&samples, arrived_ms),
    };
    let (shutdown_tx, shutdown) = watch::channel(false);
    let session = tokio::spawn(session::run(emit, shutdown));
    log(&format!(
        "recording idle for {} min; move as usual but make no gestures. Ctrl-C saves early.",
        duration.as_secs() / 60
    ));
    tokio::select! {
        () = tokio::time::sleep(duration) => {}
        () = session::shutdown_signal() => {}
    }
    shutdown_tx.send(true).ok();
    session.await?;

    let samples = std::mem::take(&mut lock(&recording).samples);
    let (Some(start_ms), Some(end_ms)) =
        (samples.first().map(|s| s.t), samples.last().map(|s| s.t))
    else {
        anyhow::bail!("no motion data arrived; nothing saved");
    };
    let seconds = (end_ms - start_ms) / 1_000;
    ensure!(
        seconds >= 30,
        "only {seconds} s of motion arrived; nothing saved"
    );
    dataset.append(&Take {
        label: Label::Idle,
        start_ms,
        samples,
    })?;
    log(&format!(
        "saved a {seconds} s idle take to {}",
        dataset.path().display()
    ));
    Ok(())
}
