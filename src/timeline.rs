use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::imu::Sample;

pub const NOMINAL_SAMPLE_PERIOD_US: u64 = 9_615;
const PRIOR_WEIGHT: i128 = 100_000;
const FIT_ARRIVALS: usize = 64;
const GAP_TOLERANCE_MS: i64 = 500;
const RETAIN_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedSample {
    pub t: u64,
    pub a: [f32; 3],
    pub g: [f32; 3],
}

#[derive(Clone, Copy)]
struct Arrival {
    samples: i64,
    at_ms: i64,
}

struct Fit {
    anchor: Arrival,
    period_ns: i128,
    earliest_ns: i128,
}

impl Fit {
    fn of(arrivals: &VecDeque<Arrival>) -> Option<Self> {
        let anchor = *arrivals.front()?;
        let points: Vec<(i128, i128)> = arrivals
            .iter()
            .map(|a| {
                (
                    i128::from(a.samples - anchor.samples),
                    i128::from(a.at_ms - anchor.at_ms) * 1_000_000,
                )
            })
            .collect();
        let count = i128::try_from(points.len()).ok()?;
        let (n, t, nt, nn) = points
            .iter()
            .fold((0, 0, 0, 0), |(n, t, nt, nn), &(samples, ns)| {
                (
                    n + samples,
                    t + ns,
                    nt + samples * ns,
                    nn + samples * samples,
                )
            });
        let prior = count * PRIOR_WEIGHT;
        let nominal_ns = i128::from(NOMINAL_SAMPLE_PERIOD_US) * 1_000;
        let covariance = count * nt - n * t;
        let variance = count * nn - n.pow(2);
        let period_ns = (covariance + prior * nominal_ns) / (variance + prior);
        let earliest_ns = points
            .iter()
            .map(|&(samples, ns)| ns - period_ns * samples)
            .min()?;
        Some(Self {
            anchor,
            period_ns,
            earliest_ns,
        })
    }

    fn at_ms(&self, samples: i64) -> i64 {
        let ns = self.earliest_ns + self.period_ns * i128::from(samples - self.anchor.samples);
        self.anchor.at_ms + i64::try_from(ns.div_euclid(1_000_000)).unwrap_or(0)
    }
}

#[derive(Default)]
pub struct Timeline {
    samples: VecDeque<TimedSample>,
    arrivals: VecDeque<Arrival>,
    delivered: i64,
}

impl Timeline {
    pub fn push(&mut self, batch: &[Sample], arrived_ms: u64) -> Vec<TimedSample> {
        let (Ok(length), Ok(at_ms)) = (i64::try_from(batch.len()), i64::try_from(arrived_ms))
        else {
            return Vec::new();
        };
        let first = self.delivered + 1;
        self.delivered += length;
        let early_ms = Fit::of(&self.arrivals).map_or(0, |fit| fit.at_ms(self.delivered) - at_ms);
        if early_ms.abs() > GAP_TOLERANCE_MS {
            self.arrivals.clear();
        }
        if early_ms > GAP_TOLERANCE_MS {
            self.samples.clear();
        }
        self.arrivals.push_back(Arrival {
            samples: self.delivered,
            at_ms,
        });
        if self.arrivals.len() > FIT_ARRIVALS {
            self.arrivals.pop_front();
        }
        let Some(fit) = Fit::of(&self.arrivals) else {
            return Vec::new();
        };
        let mut after = self.samples.back().map_or(0, |s| s.t + 1);
        let stamped: Vec<TimedSample> = (first..)
            .zip(batch)
            .map(|(samples, raw)| {
                let t = u64::try_from(fit.at_ms(samples)).unwrap_or(0).max(after);
                after = t + 1;
                TimedSample {
                    t,
                    a: raw.accel_g,
                    g: raw.gyro_dps,
                }
            })
            .collect();
        self.samples.extend(&stamped);
        let horizon = after.saturating_sub(RETAIN_MS);
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

    fn stamps(timeline: &Timeline) -> Vec<u64> {
        timeline.window(0, u64::MAX).iter().map(|s| s.t).collect()
    }

    #[test]
    fn keeps_every_sample_of_a_strap_that_outpaces_100_hz() {
        let mut timeline = Timeline::default();
        for batch_index in 0..30 {
            timeline.push(&batch(), 10_000 + batch_index * 960);
        }
        assert_eq!(stamps(&timeline).len(), 3_000);
    }

    fn worst_newest_stamp_error_ms(batch_ms: u64) -> Option<u64> {
        let mut timeline = Timeline::default();
        (0..90)
            .map(|batch_index| {
                let arrived_ms = 10_000 + batch_index * batch_ms;
                let stamped = timeline.push(&batch(), arrived_ms);
                stamped
                    .last()
                    .map_or(u64::MAX, |s| s.t.abs_diff(arrived_ms))
            })
            .skip(64)
            .max()
    }

    #[test]
    fn stamps_the_newest_sample_at_its_arrival_whatever_the_strap_rate() {
        assert_eq!(
            [930, 990].map(|ms| worst_newest_stamp_error_ms(ms) <= Some(5)),
            [true, true]
        );
    }

    #[test]
    fn a_late_batch_continues_where_the_previous_one_ended() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let late = timeline.push(&batch(), 11_150);
        assert_eq!(late.first().map(|s| s.t), Some(10_009));
    }

    #[test]
    fn a_long_gap_restarts_the_clock_at_the_arrival() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let after_gap = timeline.push(&batch(), 20_000);
        assert_eq!(after_gap.last().map(|s| s.t), Some(20_000));
    }

    #[test]
    fn an_early_batch_never_repeats_or_reverses_timestamps() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        timeline.push(&batch(), 10_500);
        let times = stamps(&timeline);
        assert_eq!(
            (times.len(), times.windows(2).all(|w| w.first() < w.last())),
            (200, true)
        );
    }

    #[test]
    fn a_clock_set_back_forgets_the_samples_stamped_ahead_of_it() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 100_000);
        timeline.push(&batch(), 50_000);
        assert_eq!(stamps(&timeline).last(), Some(&50_000));
    }

    #[test]
    fn window_returns_only_samples_inside_half_open_range() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        let times = stamps(&timeline);
        let window = timeline.window(times[10], times[20]);
        assert_eq!(
            window.iter().map(|s| s.t).collect::<Vec<_>>(),
            times[10..20]
        );
    }

    #[test]
    fn covers_is_false_until_a_batch_reaches_the_end() {
        let mut timeline = Timeline::default();
        timeline.push(&batch(), 10_000);
        assert_eq!(
            (timeline.covers(10_000), timeline.covers(10_001)),
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
