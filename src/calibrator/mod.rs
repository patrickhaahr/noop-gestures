mod guard;

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router, middleware};
use futures::{Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch};

use crate::dataset::{Dataset, Label, Take};
use crate::optical::OpticalSample;
use crate::session::{self, StrapStatus};
use crate::timeline::{SAMPLE_PERIOD_MS, TimedSample, Timeline};

const PAGE: &str = include_str!("page.html");
const COVER_TIMEOUT: Duration = Duration::from_secs(4);
const MIN_COVERAGE: f64 = 0.8;

#[derive(Clone, Debug)]
pub enum Update {
    Status(StrapStatus),
    Batch(Vec<TimedSample>),
    Optical(Vec<OpticalSample>),
}

struct AppState {
    timeline: Mutex<Timeline<TimedSample>>,
    optical: Mutex<Timeline<OpticalSample>>,
    updates: broadcast::Sender<Update>,
    status: watch::Receiver<StrapStatus>,
    shutdown: watch::Receiver<bool>,
    dataset: Dataset,
}

pub async fn run(port: u16, dataset: Dataset) -> Result<()> {
    let (updates, _) = broadcast::channel(64);
    let (status_tx, status) = watch::channel(StrapStatus::Connecting);
    let (shutdown_tx, shutdown) = watch::channel(false);
    let state = Arc::new(AppState {
        timeline: Mutex::new(Timeline::default()),
        optical: Mutex::new(Timeline::default()),
        updates: updates.clone(),
        status,
        shutdown: shutdown.clone(),
        dataset,
    });

    let session_state = Arc::clone(&state);
    let emit = move |event| match event {
        session::Event::Status(next) => {
            session_state
                .updates
                .send(Update::Status(next.clone()))
                .ok();
            status_tx.send_replace(next);
        }
        session::Event::Batch {
            samples,
            arrived_ms,
        } => {
            let stamped = lock(&session_state.timeline).push(&samples, arrived_ms);
            session_state.updates.send(Update::Batch(stamped)).ok();
        }
        session::Event::Optical {
            samples,
            arrived_ms,
        } => {
            let stamped = lock(&session_state.optical).push(&samples, arrived_ms);
            session_state.updates.send(Update::Optical(stamped)).ok();
        }
    };
    let session = tokio::spawn(session::run(emit, shutdown));

    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/events", get(events))
        .route("/takes", get(summary).post(record))
        .route("/takes/last", delete(undo))
        .layer(middleware::from_fn_with_state(port, guard::loopback_only))
        .with_state(Arc::clone(&state));
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    session::log(&format!(
        "calibrator on http://127.0.0.1:{port}  dataset {}",
        state.dataset.path().display()
    ));
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            session::shutdown_signal().await;
            shutdown_tx.send(true).ok();
        })
        .await?;
    session.await?;
    Ok(())
}

async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let current = Update::Status(state.status.borrow().clone());
    let updates = state.updates.subscribe();
    let live = futures::stream::unfold(updates, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(update) => return Some((update, rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let mut shutdown = state.shutdown.clone();
    let stream = futures::stream::once(async { current })
        .chain(live)
        .take_until(async move {
            shutdown.wait_for(|stop| *stop).await.ok();
        })
        .map(|update| Ok(to_event(&update)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_event(update: &Update) -> Event {
    let (name, json) = match update {
        Update::Status(status) => ("status", serde_json::to_string(status)),
        Update::Batch(samples) => ("batch", serde_json::to_string(samples)),
        Update::Optical(samples) => ("optical", serde_json::to_string(samples)),
    };
    Event::default()
        .event(name)
        .data(json.unwrap_or_else(|_| "null".into()))
}

#[derive(Serialize)]
struct Summary {
    labels: Vec<LabelInfo>,
    dataset: String,
}

#[derive(Serialize)]
struct LabelInfo {
    label: Label,
    window_ms: u64,
    takes: usize,
}

async fn summary(State(state): State<Arc<AppState>>) -> Response {
    summary_of(&state).map_or_else(axum::response::IntoResponse::into_response, |s| {
        Json(s).into_response()
    })
}

fn summary_of(state: &AppState) -> Result<Summary, ApiError> {
    let counts = state.dataset.counts()?;
    Ok(Summary {
        labels: Label::ALL
            .iter()
            .map(|&label| LabelInfo {
                label,
                window_ms: label.window_ms(),
                takes: counts.get(&label).copied().unwrap_or_default(),
            })
            .collect(),
        dataset: state.dataset.path().display().to_string(),
    })
}

#[derive(Deserialize)]
struct RecordRequest {
    label: Label,
    start_ms: u64,
}

async fn record(State(state): State<Arc<AppState>>, Json(req): Json<RecordRequest>) -> Response {
    let end_ms = req.start_ms + req.label.window_ms();
    let deadline = tokio::time::Instant::now() + COVER_TIMEOUT;
    while !lock(&state.timeline).covers(end_ms) {
        if tokio::time::Instant::now() >= deadline {
            return ApiError::Incomplete("no strap data reached the end of the take")
                .into_response();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let samples = lock(&state.timeline).window(req.start_ms, end_ms);
    let expected = req.label.window_ms() / SAMPLE_PERIOD_MS;
    if !covered_enough(samples.len(), expected) {
        return ApiError::Incomplete("the take has gaps; Bluetooth dropped batches")
            .into_response();
    }
    let take = Take {
        label: req.label,
        start_ms: req.start_ms,
        samples,
        optical: lock(&state.optical).window(req.start_ms, end_ms),
    };
    state
        .dataset
        .append(&take)
        .map_err(ApiError::from)
        .and_then(|()| summary_of(&state))
        .map_or_else(axum::response::IntoResponse::into_response, |s| {
            Json(s).into_response()
        })
}

fn covered_enough(got: usize, expected: u64) -> bool {
    let (Ok(got), Ok(expected)) = (u32::try_from(got), u32::try_from(expected)) else {
        return false;
    };
    f64::from(got) >= f64::from(expected) * MIN_COVERAGE
}

async fn undo(State(state): State<Arc<AppState>>) -> Response {
    state
        .dataset
        .remove_last()
        .map_err(ApiError::from)
        .and_then(|_| summary_of(&state))
        .map_or_else(axum::response::IntoResponse::into_response, |s| {
            Json(s).into_response()
        })
}

fn lock<S>(timeline: &Mutex<Timeline<S>>) -> std::sync::MutexGuard<'_, Timeline<S>> {
    timeline.lock().unwrap_or_else(PoisonError::into_inner)
}

enum ApiError {
    Io(std::io::Error),
    Incomplete(&'static str),
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Io(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            Self::Incomplete(why) => (StatusCode::UNPROCESSABLE_ENTITY, why).into_response(),
        }
    }
}
