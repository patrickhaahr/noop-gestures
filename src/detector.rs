use std::collections::VecDeque;

use crate::dataset::Label;
use crate::recognizer::{
    HALF_WINDOW, IDLE_PEAK_DPS, PEAK_RADIUS, Recognizer, features, gyro_magnitude, is_peak,
};
use crate::timeline::TimedSample;

pub const COOLDOWN_MS: u64 = 1_000;
const HISTORY: usize = 400;
const LOOKAHEAD: usize = if HALF_WINDOW > PEAK_RADIUS {
    HALF_WINDOW
} else {
    PEAK_RADIUS
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detection {
    pub label: Label,
    pub t: u64,
}

pub struct Detector {
    recognizer: Recognizer,
    buffer: VecDeque<TimedSample>,
    scanned_through: Option<u64>,
    quiet_until: u64,
}

impl Detector {
    #[must_use]
    pub const fn new(recognizer: Recognizer) -> Self {
        Self {
            recognizer,
            buffer: VecDeque::new(),
            scanned_through: None,
            quiet_until: 0,
        }
    }

    pub fn push(&mut self, stamped: &[TimedSample]) -> Vec<Detection> {
        if let Some(first) = stamped.first() {
            while self.buffer.back().is_some_and(|s| s.t >= first.t) {
                self.buffer.pop_back();
            }
        }
        self.buffer.extend(stamped);
        let samples = self.buffer.make_contiguous();
        let magnitudes: Vec<f32> = samples.iter().map(gyro_magnitude).collect();
        let ready = samples.len().saturating_sub(LOOKAHEAD);
        let mut detections = Vec::new();
        for (i, sample) in samples.iter().enumerate().take(ready) {
            if self.scanned_through.is_some_and(|t| sample.t <= t) {
                continue;
            }
            self.scanned_through = Some(sample.t);
            if sample.t < self.quiet_until || !is_peak(&magnitudes, i, IDLE_PEAK_DPS) {
                continue;
            }
            match self.recognizer.classify(&features(samples, i)) {
                Some(label @ (Label::Overview | Label::Left | Label::Right)) => {
                    self.quiet_until = sample.t + COOLDOWN_MS;
                    detections.push(Detection { label, t: sample.t });
                }
                Some(Label::Idle) | None => {}
            }
        }
        let excess = self.buffer.len().saturating_sub(HISTORY);
        self.buffer.drain(..excess);
        detections
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Take;
    use crate::recognizer::tests::{sample, swing};

    fn detector() -> Detector {
        Detector::new(Recognizer::train(&[
            swing(Label::Left, 1, 1.0),
            swing(Label::Right, 1, -1.0),
        ]))
    }

    fn stream(gestures: &[(u64, Take)]) -> Vec<TimedSample> {
        (0..600u64)
            .map(|i| {
                let g = gestures
                    .iter()
                    .filter_map(|(at, take)| {
                        take.samples.get(usize::try_from(i.checked_sub(*at)?).ok()?)
                    })
                    .fold([0.0; 3], |acc, s| {
                        [acc[0] + s.g[0], acc[1] + s.g[1], acc[2] + s.g[2]]
                    });
                sample(i * 10, g)
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
    fn a_swing_split_across_batches_fires_once() {
        let samples = stream(&[(30, swing(Label::Left, 1, 1.0))]);
        assert_eq!(
            detect(&samples),
            vec![Detection {
                label: Label::Left,
                t: 1_050
            }]
        );
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
