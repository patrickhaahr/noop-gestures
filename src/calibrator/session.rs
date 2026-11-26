use std::sync::{Arc, PoisonError};
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt as _;
use serde::Serialize;
use tokio::sync::watch;

use super::{AppState, Update, now_ms};
use crate::frame::Command;
use crate::imu::ImuBatch;
use crate::strap::Strap;

const RETRY_DELAY: Duration = Duration::from_secs(3);
const BATCH_SILENCE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum StrapStatus {
    Connecting,
    Streaming { name: String },
    Disconnected { reason: String },
}

enum SessionEnd {
    Shutdown,
    Lost(String),
}

pub async fn run(
    state: Arc<AppState>,
    status: watch::Sender<StrapStatus>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        publish(&state, &status, StrapStatus::Connecting);
        match stream_once(&state, &status, &mut shutdown.clone()).await {
            SessionEnd::Shutdown => return Ok(()),
            SessionEnd::Lost(reason) => {
                publish(&state, &status, StrapStatus::Disconnected { reason });
                tokio::select! {
                    () = tokio::time::sleep(RETRY_DELAY) => {}
                    _ = shutdown.wait_for(|stop| *stop) => return Ok(()),
                }
            }
        }
    }
}

fn publish(state: &AppState, status: &watch::Sender<StrapStatus>, next: StrapStatus) {
    state.updates.send(Update::Status(next.clone())).ok();
    status.send_replace(next);
}

async fn stream_once(
    state: &AppState,
    status: &watch::Sender<StrapStatus>,
    shutdown: &mut watch::Receiver<bool>,
) -> SessionEnd {
    match stream_until_lost(state, status, shutdown).await {
        Ok(end) => end,
        Err(e) => SessionEnd::Lost(format!("{e:#}")),
    }
}

async fn stream_until_lost(
    state: &AppState,
    status: &watch::Sender<StrapStatus>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<SessionEnd> {
    let mut strap = tokio::select! {
        strap = Strap::connect() => strap?,
        _ = shutdown.wait_for(|stop| *stop) => return Ok(SessionEnd::Shutdown),
    };
    let mut frames = strap.frames().await?;
    strap.send(Command::RealtimeRawStream { on: true }).await?;
    publish(
        state,
        status,
        StrapStatus::Streaming {
            name: strap.name.clone(),
        },
    );
    let end = loop {
        let frame = tokio::select! {
            next = tokio::time::timeout(BATCH_SILENCE, frames.next()) => next,
            _ = shutdown.wait_for(|stop| *stop) => break SessionEnd::Shutdown,
        };
        match frame {
            Ok(Some(frame)) => {
                if let Some(batch) = ImuBatch::decode(&frame) {
                    let stamped = state
                        .timeline
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(&batch.samples, now_ms());
                    state.updates.send(Update::Batch(stamped)).ok();
                }
            }
            Ok(None) => break SessionEnd::Lost("strap disconnected".into()),
            Err(_) => break SessionEnd::Lost("no IMU data for 5 s".into()),
        }
    };
    strap
        .send(Command::RealtimeRawStream { on: false })
        .await
        .ok();
    strap.disconnect().await.ok();
    Ok(end)
}
