use std::collections::VecDeque;

use crate::dataset::Label;
use crate::recognizer::{
    EARLY_MIN_POST, PEAK_ACTIVITY, POST_SAMPLES, RISING_WINDOW, Recognizer, activity, features,
    is_peak, is_rising_tail, tail_features,
};
use crate::timeline::TimedSample;

pub const COOLDOWN_SAMPLES: u64 = 100;
const HISTORY: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    FullWindow,
    Prefix { samples_past_peak: usize },
    RisingEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detection {
    pub label: Label,
    pub evidence: Evidence,
    pub t: u64,
    pub index_in_batch: u64,
    pub batches_waited: usize,
}

pub struct Detector {
    recognizer: Recognizer,
    buffer: VecDeque<TimedSample>,
    batch_starts: VecDeque<u64>,
    received: u64,
    scanned: u64,
    quiet_until: u64,
}

impl Detector {
    #[must_use]
    pub const fn new(recognizer: Recognizer) -> Self {
        Self {
            recognizer,
            buffer: VecDeque::new(),
            batch_starts: VecDeque::new(),
            received: 0,
            scanned: 0,
            quiet_until: 0,
        }
    }

    pub fn push(&mut self, batch: &[TimedSample]) -> Vec<Detection> {
        self.batch_starts.push_back(self.received);
        self.buffer.extend(batch);
        self.received += count(batch.len());
        let samples = self.buffer.make_contiguous();
        let oldest = self.received - count(samples.len());
        let magnitudes: Vec<f32> = samples.iter().map(activity).collect();
        let ready = samples.len().saturating_sub(POST_SAMPLES);
        let early_ready = samples.len().saturating_sub(EARLY_MIN_POST);
        let mut detections = Vec::new();
        for (i, (sequence, sample)) in (oldest..).zip(samples.iter()).enumerate().take(early_ready)
        {
            if sequence < self.scanned.max(self.quiet_until)
                || !is_peak(&magnitudes, i, PEAK_ACTIVITY)
            {
                continue;
            }
            let (label, evidence) = if i < ready {
                let window = features(samples, i, POST_SAMPLES);
                (self.recognizer.classify(&window), Evidence::FullWindow)
            } else {
                let samples_past_peak = samples.len() - 1 - i;
                let prefix = features(samples, i, samples_past_peak);
                (
                    self.recognizer.classify_early(&prefix),
                    Evidence::Prefix { samples_past_peak },
                )
            };
            let Some(label) = label.filter(|&l| l != Label::Idle) else {
                continue;
            };
            self.quiet_until = sequence + COOLDOWN_SAMPLES;
            detections.push(make_detection(
                &self.batch_starts,
                label,
                evidence,
                sequence,
                sample.t,
                oldest,
            ));
        }
        if let Some((tail_index, tail_t)) = samples.len().checked_sub(1).and_then(|i| {
            let sample = samples.get(i)?;
            (samples.len() >= RISING_WINDOW).then_some((i, sample.t))
        }) {
            let tail_sequence = oldest + count(tail_index);
            if tail_sequence >= self.quiet_until && is_rising_tail(&magnitudes, tail_index) {
                let tail = tail_features(samples, tail_index);
                if let Some(label) = self
                    .recognizer
                    .classify_rising(&tail)
                    .filter(|&l| l != Label::Idle)
                {
                    self.quiet_until = tail_sequence + COOLDOWN_SAMPLES;
                    detections.push(make_detection(
                        &self.batch_starts,
                        label,
                        Evidence::RisingEdge,
                        tail_sequence,
                        tail_t,
                        oldest,
                    ));
                }
            }
        }
        self.scanned = self.scanned.max(oldest + count(ready));
        let excess = self.buffer.len().saturating_sub(HISTORY);
        self.buffer.drain(..excess);
        let oldest = self.received - count(self.buffer.len());
        while self.batch_starts.get(1).is_some_and(|&next| next <= oldest) {
            self.batch_starts.pop_front();
        }
        detections
    }
}

fn make_detection(
    batch_starts: &VecDeque<u64>,
    label: Label,
    evidence: Evidence,
    sequence: u64,
    t: u64,
    oldest: u64,
) -> Detection {
    let batches_waited = batch_starts
        .iter()
        .rev()
        .take_while(|&&start| start > sequence)
        .count();
    let batch_start = batch_starts
        .iter()
        .rev()
        .nth(batches_waited)
        .copied()
        .unwrap_or(oldest);
    Detection {
        label,
        evidence,
        t,
        index_in_batch: sequence - batch_start,
        batches_waited,
    }
}

fn count(samples: usize) -> u64 {
    u64::try_from(samples).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Take;
    use crate::recognizer::tests::{knock, sample, swing};

    fn detector() -> Detector {
        Detector::new(Recognizer::train(&[
            swing(Label::Left, 1, 1.0),
            swing(Label::Right, 1, -1.0),
            knock(),
        ]))
    }

    fn stream(gestures: &[(u64, Take)]) -> Vec<TimedSample> {
        (0..600u64)
            .map(|i| {
                let overlapping: Vec<&TimedSample> = gestures
                    .iter()
                    .filter_map(|(at, take)| {
                        take.samples.get(usize::try_from(i.checked_sub(*at)?).ok()?)
                    })
                    .collect();
                let g = overlapping.iter().fold([0.0; 3], |acc, s| {
                    [acc[0] + s.g[0], acc[1] + s.g[1], acc[2] + s.g[2]]
                });
                let jolt: f32 = overlapping.iter().map(|s| s.a[2] - 1.0).sum();
                TimedSample {
                    a: [0.0, 0.0, 1.0 + jolt],
                    ..sample(i * 10, g)
                }
            })
            .collect()
    }

    fn detect(samples: &[TimedSample]) -> Vec<Detection> {
        let mut detector = detector();
        samples
            .chunks(100)
            .flat_map(|batch| detector.push(batch))
            .collect()
    }

    #[test]
    fn a_swing_split_across_batches_fires_on_the_rising_edge_and_only_once() {
        let samples = stream(&[(30, swing(Label::Left, 1, 1.0))]);
        assert_eq!(
            detect(&samples),
            vec![Detection {
                label: Label::Left,
                evidence: Evidence::RisingEdge,
                t: 990,
                index_in_batch: 99,
                batches_waited: 0,
            }]
        );
    }

    #[test]
    fn a_confident_peak_near_the_batch_end_fires_early_and_only_once() {
        let samples = stream(&[(115, swing(Label::Left, 1, 1.0))]);
        assert_eq!(
            detect(&samples),
            vec![Detection {
                label: Label::Left,
                evidence: Evidence::Prefix {
                    samples_past_peak: 9
                },
                t: 1_900,
                index_in_batch: 90,
                batches_waited: 0,
            }]
        );
    }

    #[test]
    fn a_confident_peak_one_sample_before_the_batch_end_fires_early() {
        let samples = stream(&[(123, swing(Label::Left, 1, 1.0))]);
        assert_eq!(
            detect(&samples),
            vec![Detection {
                label: Label::Left,
                evidence: Evidence::Prefix {
                    samples_past_peak: 1
                },
                t: 1_980,
                index_in_batch: 98,
                batches_waited: 0,
            }]
        );
    }

    #[test]
    fn an_ambiguous_peak_near_the_batch_end_waits_for_the_full_window() {
        let diverging_after_the_peak = |label, tail: f32| {
            let mut take = swing(label, 1, 1.0);
            for late in take.samples.iter_mut().skip(86) {
                late.g[0] = tail;
            }
            take
        };
        let left = diverging_after_the_peak(Label::Left, 100.0);
        let right = diverging_after_the_peak(Label::Right, -100.0);
        let mut detector = Detector::new(Recognizer::train(&[left.clone(), right]));
        let fired: Vec<Detection> = stream(&[(115, left)])
            .chunks(100)
            .flat_map(|batch| detector.push(batch))
            .collect();
        assert_eq!(
            fired,
            vec![Detection {
                label: Label::Left,
                evidence: Evidence::FullWindow,
                t: 1_900,
                index_in_batch: 90,
                batches_waited: 1,
            }]
        );
    }

    #[test]
    fn a_swing_fires_when_timestamps_step_back_over_its_peak() {
        let mut samples = stream(&[(100, swing(Label::Left, 1, 1.0))]);
        for late in samples.iter_mut().skip(200) {
            late.t -= 400;
        }
        let labels: Vec<Label> = detect(&samples).iter().map(|d| d.label).collect();
        assert_eq!(labels, vec![Label::Left]);
    }

    #[test]
    fn a_knock_without_rotation_fires() {
        let samples = stream(&[(100, knock())]);
        let labels: Vec<Label> = detect(&samples).iter().map(|d| d.label).collect();
        assert_eq!(labels, vec![Label::Knock]);
    }

    #[test]
    fn a_return_swing_inside_the_cooldown_is_ignored() {
        let samples = stream(&[
            (100, swing(Label::Left, 1, 1.0)),
            (170, swing(Label::Right, 1, -1.0)),
        ]);
        assert_eq!(detect(&samples).len(), 1);
    }

    #[test]
    fn a_swing_after_the_cooldown_fires_again() {
        let samples = stream(&[
            (100, swing(Label::Left, 1, 1.0)),
            (300, swing(Label::Right, 1, -1.0)),
        ]);
        let labels: Vec<Label> = detect(&samples).iter().map(|d| d.label).collect();
        assert_eq!(labels, vec![Label::Left, Label::Right]);
    }
}
