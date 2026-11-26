use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::imu::Sample;

pub const SAMPLE_PERIOD_MS: u64 = 10;
const RESYNC_TOLERANCE_MS: u64 = 400;
const RETAIN_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedSample {
    pub t: u64,
    pub a: [f32; 3],
    pub g: [f32; 3],
}

#[derive(Default)]
pub struct Timeline {
    samples: VecDeque<TimedSample>,
    next_ms: Option<u64>,
}

impl Timeline {
    pub fn push(&mut self, batch: &[Sample], arrived_ms: u64) -> Vec<TimedSample> {
        let span_ms = u64::try_from(batch.len()).unwrap_or(0) * SAMPLE_PERIOD_MS;
        let arrival_start = arrived_ms.saturating_sub(span_ms);
        let start = match self.next_ms {
            Some(next) if next.abs_diff(arrival_start) <= RESYNC_TOLERANCE_MS => next,
            _ => arrival_start,
        };
        let stamped: Vec<TimedSample> = (start..)
            .step_by(usize::try_from(SAMPLE_PERIOD_MS).unwrap_or(1))
            .zip(batch)
            .map(|(t, s)| TimedSample {
                t,
                a: s.accel_g,
                g: s.gyro_dps,
            })
            .collect();
        self.next_ms = Some(start + span_ms);
        while self.samples.back().is_some_and(|s| s.t >= start) {
            self.samples.pop_back();
        }
        self.samples.extend(&stamped);
        let horizon = (start + span_ms).saturating_sub(RETAIN_MS);
        while self.samples.front().is_some_and(|s| s.t < horizon) {
            self.samples.pop_front();
        }
        stamped
    }

    #[must_use]
    pub fn covers(&self, end_ms: u64) -> bool {
        self.samples.back().is_some_and(|s| s.t >= end_ms)
    }

    #[must_use]
    pub fn window(&self, start_ms: u64, end_ms: u64) -> Vec<TimedSample> {
        self.samples
            .iter()
            .filter(|s| (start_ms..end_ms).contains(&s.t))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> Vec<Sample> {
        vec![Sample::default(); 100]
    }

    #[test]
    fn jittered_arrivals_stay_contiguous() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let second = timeline.push(&batch(), 11_150);
        assert_eq!(second.first().map(|s| s.t), Some(10_000));
    }

    #[test]
    fn long_gap_resyncs_to_arrival_time() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let after_gap = timeline.push(&batch(), 20_000);
        assert_eq!(after_gap.first().map(|s| s.t), Some(19_000));
    }

    #[test]
    fn early_arrival_resync_never_repeats_timestamps() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        timeline.push(&batch(), 10_500);
        let times: Vec<u64> = timeline.window(0, 20_000).iter().map(|s| s.t).collect();
        assert!(times.windows(2).all(|w| w.first() < w.last()));
    }

    #[test]
    fn window_returns_only_samples_inside_half_open_range() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let window = timeline.window(9_500, 9_600);
        assert_eq!(
            (window.len(), window.first().map(|s| s.t)),
            (10, Some(9_500))
        );
    }

    #[test]
    fn covers_is_false_until_a_batch_reaches_the_end() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        assert_eq!(
            (timeline.covers(9_990), timeline.covers(10_000)),
            (true, false)
        );
    }

    #[test]
    fn drops_samples_older_than_retention() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        timeline.push(&batch(), 50_000);
        assert_eq!(timeline.window(0, 40_000), Vec::<TimedSample>::new());
    }
}
