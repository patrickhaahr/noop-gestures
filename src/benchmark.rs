use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::dataset::{Label, Take};
use crate::detector::Detector;
use crate::imu::Sample;
use crate::recognizer::{Recognizer, monotonic};
use crate::timeline::{TimedSample, Timeline};

const BATCH_SAMPLES: usize = 100;
const STRAP_SAMPLE_PERIOD_US: u64 = 9_600;
const STREAM_START_US: u64 = 1_000_000_000;
const TRANSPORT_FLOOR_US: u64 = 60_000;
const TRANSPORT_JITTER_US: u64 = 60_000;
const LEAD_SAMPLES: usize = 2_000;
const TAIL_SAMPLES: usize = 300;
const PLACEMENTS: usize = 28;
const PLACEMENT_STEP: usize = 37;
const LONG_IDLE_SAMPLES: usize = 6_000;

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn signed_ms(us: u64) -> i64 {
    i64::try_from(us / 1_000).unwrap_or(i64::MAX)
}

fn sampled_at_us(index: usize) -> u64 {
    STREAM_START_US + count(index) * STRAP_SAMPLE_PERIOD_US
}

fn transport_us(batch: usize) -> u64 {
    let scrambled = count(batch).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
    TRANSPORT_FLOOR_US + scrambled % TRANSPORT_JITTER_US
}

struct Fire {
    label: Label,
    latency_ms: i64,
    logged_ms: i64,
}

fn replay(recognizer: Recognizer, samples: &[TimedSample]) -> Vec<Fire> {
    let mut timeline = Timeline::default();
    let mut detector = Detector::new(recognizer);
    let mut sampled_us: HashMap<u64, u64> = HashMap::new();
    let mut fires = Vec::new();
    for (batch, chunk) in samples.chunks(BATCH_SAMPLES).enumerate() {
        let raw: Vec<Sample> = chunk
            .iter()
            .map(|s| Sample {
                accel_g: s.a,
                gyro_dps: s.g,
            })
            .collect();
        let first = batch * BATCH_SAMPLES;
        let arrived_us = sampled_at_us(first + chunk.len() - 1) + transport_us(batch);
        let stamped = timeline.push(&raw, arrived_us / 1_000);
        for (offset, sample) in stamped.iter().enumerate() {
            sampled_us.insert(sample.t, sampled_at_us(first + offset));
        }
        for detection in detector.push(&stamped) {
            let Some(&peak_us) = sampled_us.get(&detection.t) else {
                continue;
            };
            fires.push(Fire {
                label: detection.label,
                latency_ms: signed_ms(arrived_us) - signed_ms(peak_us),
                logged_ms: signed_ms(arrived_us) - i64::try_from(detection.t).unwrap_or(i64::MAX),
            });
        }
    }
    fires
}

fn placed(samples: &[TimedSample], lead: usize) -> Vec<TimedSample> {
    let (Some(&first), Some(&last)) = (samples.first(), samples.last()) else {
        return Vec::new();
    };
    let unpadded = lead + samples.len() + TAIL_SAMPLES;
    let tail = TAIL_SAMPLES + unpadded.next_multiple_of(BATCH_SAMPLES) - unpadded;
    std::iter::repeat_n(first, lead)
        .chain(samples.iter().copied())
        .chain(std::iter::repeat_n(last, tail))
        .collect()
}

#[derive(Default)]
struct Tally {
    replays: usize,
    recognised: usize,
    wrong: usize,
    missed: usize,
    extra: usize,
    latencies_ms: Vec<i64>,
    logging_errors_ms: Vec<i64>,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        self.replays += other.replays;
        self.recognised += other.recognised;
        self.wrong += other.wrong;
        self.missed += other.missed;
        self.extra += other.extra;
        self.latencies_ms.extend(&other.latencies_ms);
        self.logging_errors_ms.extend(&other.logging_errors_ms);
    }

    fn record(&mut self, expected: Label, fires: &[Fire]) {
        self.replays += 1;
        self.extra += fires.len().saturating_sub(1);
        match fires.first() {
            None if expected == Label::Idle => self.recognised += 1,
            None => self.missed += 1,
            Some(fire) if fire.label == expected => {
                self.recognised += 1;
                self.latencies_ms.push(fire.latency_ms);
            }
            Some(_) => self.wrong += 1,
        }
        self.logging_errors_ms
            .extend(fires.iter().map(|f| f.logged_ms - f.latency_ms));
    }
}

fn percentile(values: &[i64], percent: usize) -> Option<i64> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted
        .get((sorted.len().saturating_sub(1) * percent).div_ceil(100))
        .copied()
}

fn mean(values: &[i64]) -> Option<i64> {
    let n = i64::try_from(values.len()).ok().filter(|&n| n > 0)?;
    Some(values.iter().sum::<i64>() / n)
}

struct IdleReplay {
    seconds: u64,
    fired: BTreeMap<Label, usize>,
}

pub struct Benchmark {
    gestures: BTreeMap<Label, Tally>,
    long_idle: Vec<IdleReplay>,
}

#[must_use]
pub fn run(takes: &[Take]) -> Benchmark {
    let recognizer = Recognizer::train(takes);
    let mut gestures: BTreeMap<Label, Tally> = BTreeMap::new();
    let mut long_idle = Vec::new();
    for (index, take) in takes.iter().enumerate() {
        let held_out = recognizer.holding_out(index);
        let samples = monotonic(&take.samples);
        if take.label == Label::Idle && samples.len() >= LONG_IDLE_SAMPLES {
            let mut fired = BTreeMap::new();
            for fire in replay(held_out, &placed(&samples, 0)) {
                *fired.entry(fire.label).or_default() += 1;
            }
            long_idle.push(IdleReplay {
                seconds: count(samples.len()) * STRAP_SAMPLE_PERIOD_US / 1_000_000,
                fired,
            });
            continue;
        }
        let tally = gestures.entry(take.label).or_default();
        for placement in 0..PLACEMENTS {
            let lead = LEAD_SAMPLES + placement * PLACEMENT_STEP;
            tally.record(
                take.label,
                &replay(held_out.clone(), &placed(&samples, lead)),
            );
        }
    }
    Benchmark {
        gestures,
        long_idle,
    }
}

fn cell(value: Option<i64>) -> String {
    value.map_or_else(|| "-".into(), |v| v.to_string())
}

fn row(f: &mut fmt::Formatter<'_>, name: &str, tally: &Tally) -> fmt::Result {
    writeln!(
        f,
        "{:>14}{:>9}{:>12}{:>7}{:>8}{:>7}{:>13}{:>13}{:>14}{:>17}{:>17}{:>17}",
        name,
        tally.replays,
        tally.recognised,
        tally.wrong,
        tally.missed,
        tally.extra,
        cell(percentile(&tally.latencies_ms, 50)),
        cell(percentile(&tally.latencies_ms, 95)),
        cell(mean(&tally.latencies_ms)),
        cell(percentile(&tally.logging_errors_ms, 0)),
        cell(percentile(&tally.logging_errors_ms, 50)),
        cell(percentile(&tally.logging_errors_ms, 100)),
    )
}

impl fmt::Display for Benchmark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "each take replayed at {PLACEMENTS} placements, held out, as {BATCH_SAMPLES}-sample batches from a strap sampling every {STRAP_SAMPLE_PERIOD_US} us, each batch arriving {}-{} ms after its last sample",
            TRANSPORT_FLOOR_US / 1_000,
            (TRANSPORT_FLOOR_US + TRANSPORT_JITTER_US) / 1_000
        )?;
        writeln!(
            f,
            "latency is peak sample to the batch that fires it; logged-true is the daemon's logged latency minus that"
        )?;
        writeln!(
            f,
            "{:>14}{:>9}{:>12}{:>7}{:>8}{:>7}{:>13}{:>13}{:>14}{:>17}{:>17}{:>17}",
            "take",
            "replays",
            "recognised",
            "wrong",
            "missed",
            "extra",
            "latency p50",
            "latency p95",
            "latency mean",
            "logged-true min",
            "logged-true p50",
            "logged-true max"
        )?;
        let mut all_gestures = Tally::default();
        for (label, tally) in &self.gestures {
            row(f, label.name(), tally)?;
            if *label != Label::Idle {
                all_gestures.absorb(tally);
            }
        }
        row(f, "all gestures", &all_gestures)?;
        for idle in &self.long_idle {
            let total: usize = idle.fired.values().sum();
            let detail: Vec<String> = idle
                .fired
                .iter()
                .map(|(label, n)| format!("{} {n}", label.name()))
                .collect();
            writeln!(
                f,
                "idle take of {} min {} s: {total} false fires ({})",
                idle.seconds / 60,
                idle.seconds % 60,
                detail.join(", ")
            )?;
        }
        Ok(())
    }
}
