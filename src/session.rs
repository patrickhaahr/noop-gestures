use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use futures::StreamExt as _;
use serde::Serialize;
use tokio::sync::watch;

use crate::frame::Command;
use crate::imu::{ImuBatch, Sample};
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

pub enum Event {
    Status(StrapStatus),
    Batch {
        samples: Vec<Sample>,
        arrived_ms: u64,
    },
}

enum SessionEnd {
    Shutdown,
    Lost(String),
}

struct ArrivalClock {
    connected: Instant,
    connected_unix_ms: u64,
}

impl ArrivalClock {
    fn start() -> Self {
        Self {
            connected: Instant::now(),
            connected_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        }
    }

    fn now_ms(&self) -> u64 {
        let elapsed = u64::try_from(self.connected.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.connected_unix_ms.saturating_add(elapsed)
    }
}

pub fn log(line: &str) {
    use std::io::Write as _;
    writeln!(std::io::stderr(), "{line}").ok();
}

pub async fn shutdown_signal() {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
}

pub async fn run(mut emit: impl FnMut(Event) + Send, mut shutdown: watch::Receiver<bool>) {
    loop {
        emit(Event::Status(StrapStatus::Connecting));
        match stream_once(&mut emit, &mut shutdown.clone()).await {
            SessionEnd::Shutdown => return,
            SessionEnd::Lost(reason) => {
                emit(Event::Status(StrapStatus::Disconnected { reason }));
                tokio::select! {
                    () = tokio::time::sleep(RETRY_DELAY) => {}
                    _ = shutdown.wait_for(|stop| *stop) => return,
                }
            }
        }
    }
}

async fn stream_once(
    emit: &mut (impl FnMut(Event) + Send),
    shutdown: &mut watch::Receiver<bool>,
) -> SessionEnd {
    match stream_until_lost(emit, shutdown).await {
        Ok(end) => end,
        Err(e) => SessionEnd::Lost(format!("{e:#}")),
    }
}

async fn stream_until_lost(
    emit: &mut (impl FnMut(Event) + Send),
    shutdown: &mut watch::Receiver<bool>,
) -> Result<SessionEnd> {
    let mut strap = tokio::select! {
        strap = Strap::connect() => strap?,
        _ = shutdown.wait_for(|stop| *stop) => return Ok(SessionEnd::Shutdown),
    };
    let clock = ArrivalClock::start();
    let mut frames = strap.frames().await?;
    strap.send(Command::RealtimeRawStream { on: true }).await?;
    emit(Event::Status(StrapStatus::Streaming {
        name: strap.name.clone(),
    }));
    let end = loop {
        let frame = tokio::select! {
            next = tokio::time::timeout(BATCH_SILENCE, frames.next()) => next,
            _ = shutdown.wait_for(|stop| *stop) => break SessionEnd::Shutdown,
        };
        match frame {
            Ok(Some(frame)) => {
                if let Some(batch) = ImuBatch::decode(&frame) {
                    emit(Event::Batch {
                        samples: batch.samples,
                        arrived_ms: clock.now_ms(),
                    });
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
