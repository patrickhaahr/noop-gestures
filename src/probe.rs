use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use btleplug::api::ValueNotification;
use futures::StreamExt as _;
use uuid::Uuid;

use crate::frame::{Command, Frame, Reassembler};
use crate::imu::ImuBatch;
use crate::strap::Strap;

const REALTIME_RAW_PACKET_TYPE: u8 = 43;
const COMMAND_RESPONSE_PACKET_TYPE: u8 = 36;
const NEXT_FEATURE_FLAG_NAME: u8 = 118;
const FEATURE_FLAG_NAMES_END: u8 = 0xFF;
const MAX_FEATURE_FLAG_NAMES: usize = 24;
const QUERY_SPACING: Duration = Duration::from_millis(700);

fn magnitude(v: [f32; 3]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").ok();
        out
    })
}

fn read_only_queries() -> VecDeque<Command> {
    [
        Command::GetImuDataStream,
        Command::GetR12RealtimeStream,
        Command::GetPacketConfig,
        Command::StartFeatureFlagNames,
    ]
    .into_iter()
    .chain(std::iter::repeat_n(
        Command::NextFeatureFlagName,
        MAX_FEATURE_FLAG_NAMES,
    ))
    .collect()
}

fn ends_feature_flag_names(frame: &Frame) -> bool {
    let bytes = frame.bytes();
    frame.packet_type() == COMMAND_RESPONSE_PACKET_TYPE
        && bytes.get(6) == Some(&NEXT_FEATURE_FLAG_NAME)
        && bytes.get(10) == Some(&FEATURE_FLAG_NAMES_END)
}

#[derive(Default)]
struct Assembly {
    reassembler: Reassembler,
    first_fragment: Option<Instant>,
    fragments: usize,
}

struct Delivery {
    at: Instant,
    fragments: usize,
    spread: Duration,
}

struct Report {
    out: std::io::Stdout,
    start: Instant,
    assemblies: HashMap<Uuid, Assembly>,
    fragment_sizes: BTreeMap<(u16, usize), u32>,
    by_type: BTreeMap<(u8, usize), u32>,
    last_batch: Option<Instant>,
}

impl Report {
    fn feed(&mut self, fragment: &ValueNotification) -> Option<(Vec<Frame>, Delivery)> {
        let at = Instant::now();
        let [low, high, ..] = fragment.uuid.as_fields().0.to_le_bytes();
        *self
            .fragment_sizes
            .entry((u16::from_le_bytes([low, high]), fragment.value.len()))
            .or_default() += 1;
        let assembly = self.assemblies.entry(fragment.uuid).or_default();
        let first_fragment = *assembly.first_fragment.get_or_insert(at);
        assembly.fragments += 1;
        let frames = assembly.reassembler.feed(&fragment.value);
        if frames.is_empty() {
            return None;
        }
        assembly.first_fragment = None;
        let delivery = Delivery {
            at,
            fragments: std::mem::take(&mut assembly.fragments),
            spread: at.duration_since(first_fragment),
        };
        Some((frames, delivery))
    }

    fn frame(&mut self, frame: &Frame, delivery: &Delivery) -> Result<()> {
        let t_ms = delivery.at.duration_since(self.start).as_millis();
        *self
            .by_type
            .entry((frame.packet_type(), frame.bytes().len()))
            .or_default() += 1;
        if frame.packet_type() != REALTIME_RAW_PACKET_TYPE {
            writeln!(
                self.out,
                "t={t_ms:>6}ms type={} {}",
                frame.packet_type(),
                hex(frame.bytes())
            )?;
        }
        let Some(batch) = ImuBatch::decode(frame) else {
            return Ok(());
        };
        let gap_ms = self
            .last_batch
            .map_or(0, |t| delivery.at.duration_since(t).as_millis());
        self.last_batch = Some(delivery.at);
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
        writeln!(
            self.out,
            "t={t_ms:>6}ms gap={gap_ms:>5}ms fragments={} spread={}ms strap_ts={} n={} |a|max={accel_peak:.2}g |w|max={gyro_peak:.0}dps",
            delivery.fragments,
            delivery.spread.as_millis(),
            batch.strap_unix_s,
            batch.samples.len(),
        )?;
        Ok(())
    }
}

pub async fn run(seconds: u64, imu_stream: bool, queries: bool) -> Result<()> {
    let mut strap = Strap::connect().await?;
    let mut report = Report {
        out: std::io::stdout(),
        start: Instant::now(),
        assemblies: HashMap::new(),
        fragment_sizes: BTreeMap::new(),
        by_type: BTreeMap::new(),
        last_batch: None,
    };
    writeln!(
        report.out,
        "connected to {}, ATT MTU {}",
        strap.name,
        strap.mtu()
    )?;
    let mut fragments = strap.fragments().await?;
    strap.send(Command::RealtimeRawStream { on: true }).await?;
    if imu_stream {
        strap.send(Command::ImuDataStream { on: true }).await?;
    }
    let mut pending = if queries {
        read_only_queries()
    } else {
        VecDeque::new()
    };

    let deadline = report.start + Duration::from_secs(seconds);
    let mut next_query = report.start + QUERY_SPACING;
    loop {
        let wake = if pending.is_empty() {
            deadline
        } else {
            deadline.min(next_query)
        };
        let fragment = match tokio::time::timeout_at(wake.into(), fragments.next()).await {
            Ok(Some(fragment)) => fragment,
            Ok(None) => break,
            Err(_) if wake == deadline => break,
            Err(_) => {
                next_query += QUERY_SPACING;
                if let Some(query) = pending.pop_front() {
                    writeln!(
                        report.out,
                        "t={:>6}ms sending {query:?}",
                        report.start.elapsed().as_millis()
                    )?;
                    strap.send(query).await?;
                }
                continue;
            }
        };
        let Some((frames, delivery)) = report.feed(&fragment) else {
            continue;
        };
        for frame in frames {
            report.frame(&frame, &delivery)?;
            if ends_feature_flag_names(&frame) {
                pending.clear();
            }
        }
    }

    if imu_stream {
        strap.send(Command::ImuDataStream { on: false }).await?;
    }
    strap.send(Command::RealtimeRawStream { on: false }).await?;
    writeln!(
        report.out,
        "fragments by (characteristic, bytes): {:?}",
        report.fragment_sizes
    )?;
    writeln!(
        report.out,
        "frames by (packet type, bytes): {:?}",
        report.by_type
    )?;
    strap.disconnect().await?;
    Ok(())
}
