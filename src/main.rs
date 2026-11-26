mod frame;
mod imu;
mod strap;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::StreamExt as _;

use crate::frame::Command;
use crate::imu::ImuBatch;
use crate::strap::Strap;

struct ProbeArgs {
    seconds: u64,
    imu_stream: bool,
}

fn parse_args() -> ProbeArgs {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds = args.iter().find_map(|a| a.parse().ok()).unwrap_or(15);
    ProbeArgs {
        seconds,
        imu_stream: args.iter().any(|a| a == "--imu-stream"),
    }
}

fn magnitude(v: [f32; 3]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();
    let mut out = std::io::stdout().lock();
    let mut strap = Strap::connect().await?;
    writeln!(out, "connected to {}", strap.name)?;
    let mut frames = strap.frames().await?;
    strap.send(Command::RealtimeRawStream { on: true }).await?;
    if args.imu_stream {
        strap.send(Command::ImuDataStream { on: true }).await?;
    }

    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.seconds);
    let mut by_type: BTreeMap<(u8, usize), u32> = BTreeMap::new();
    let mut last_batch: Option<Instant> = None;
    while let Ok(Some(frame)) = tokio::time::timeout_at(deadline.into(), frames.next()).await {
        *by_type
            .entry((frame.packet_type(), frame.bytes().len()))
            .or_default() += 1;
        let Some(batch) = ImuBatch::decode(&frame) else {
            continue;
        };
        let now = Instant::now();
        let gap_ms = last_batch.map_or(0, |t| now.duration_since(t).as_millis());
        last_batch = Some(now);
        let accel_peak = batch
            .samples
            .iter()
            .map(|s| magnitude(s.accel_g))
            .fold(0.0, f32::max);
        let gyro_peak = batch
            .samples
            .iter()
            .map(|s| magnitude(s.gyro_dps))
            .fold(0.0, f32::max);
        let first = batch.samples.first().copied().unwrap_or_default();
        writeln!(
            out,
            "t={:>6}ms gap={gap_ms:>5}ms strap_ts={} n={} a0=[{:+.2} {:+.2} {:+.2}]g |a|max={accel_peak:.2}g |w|max={gyro_peak:.0}dps",
            now.duration_since(start).as_millis(),
            batch.strap_unix_s,
            batch.samples.len(),
            first.accel_g[0],
            first.accel_g[1],
            first.accel_g[2],
        )?;
    }

    if args.imu_stream {
        strap.send(Command::ImuDataStream { on: false }).await?;
    }
    strap.send(Command::RealtimeRawStream { on: false }).await?;
    writeln!(out, "frames by (packet type, bytes): {by_type:?}")?;
    strap.disconnect().await?;
    Ok(())
}
