pub mod evaluate;

use crate::dataset::{Label, Take};
use crate::timeline::TimedSample;

pub const HALF_WINDOW: usize = 60;
pub const PEAK_RADIUS: usize = 60;
pub const PEAK_ACTIVITY: f32 = 60.0;
const GYRO_SCALE_DPS: f32 = 200.0;
const DPS_PER_G_OF_JOLT: f32 = 50.0;

fn norm(v: [f32; 3]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[must_use]
pub fn activity(sample: &TimedSample) -> f32 {
    DPS_PER_G_OF_JOLT.mul_add((norm(sample.a) - 1.0).abs(), norm(sample.g))
}

#[must_use]
pub fn is_peak(magnitudes: &[f32], at: usize, threshold: f32) -> bool {
    let Some(&peak) = magnitudes.get(at) else {
        return false;
    };
    let before = magnitudes
        .get(at.saturating_sub(PEAK_RADIUS)..at)
        .unwrap_or_default();
    let after = magnitudes
        .get(at + 1..magnitudes.len().min(at + 1 + PEAK_RADIUS))
        .unwrap_or_default();
    peak >= threshold && before.iter().all(|&m| m < peak) && after.iter().all(|&m| m <= peak)
}

#[must_use]
pub fn features(samples: &[TimedSample], centre: usize) -> Vec<f32> {
    let last = samples.len().saturating_sub(1);
    (0..2 * HALF_WINDOW)
        .map(|k| (centre + k).saturating_sub(HALF_WINDOW).min(last))
        .filter_map(|i| samples.get(i))
        .flat_map(|s| {
            let [gx, gy, gz] = s.g.map(|v| v / GYRO_SCALE_DPS);
            [gx, gy, gz, s.a[0], s.a[1], s.a[2]]
        })
        .collect()
}

fn monotonic(samples: &[TimedSample]) -> Vec<TimedSample> {
    let mut sorted: Vec<TimedSample> = samples.iter().rev().copied().collect();
    sorted.sort_by_key(|s| s.t);
    sorted.dedup_by_key(|s| s.t);
    sorted
}

#[must_use]
pub fn segments(take: &Take) -> Vec<Vec<f32>> {
    let samples = monotonic(&take.samples);
    let magnitudes: Vec<f32> = samples.iter().map(activity).collect();
    let centres: Vec<usize> = match take.label {
        Label::Idle => (0..samples.len())
            .filter(|&i| is_peak(&magnitudes, i, PEAK_ACTIVITY))
            .collect(),
        Label::Overview
        | Label::Left
        | Label::Right
        | Label::WorkspaceUp
        | Label::WorkspaceDown
        | Label::ReverseTwist
        | Label::Knock => magnitudes
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .into_iter()
            .collect(),
    };
    centres.into_iter().map(|c| features(&samples, c)).collect()
}

struct Template {
    label: Label,
    take: usize,
    features: Vec<f32>,
}

pub struct Recognizer {
    templates: Vec<Template>,
}

fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

impl Recognizer {
    #[must_use]
    pub fn train<'a>(takes: impl IntoIterator<Item = &'a Take>) -> Self {
        let templates = takes
            .into_iter()
            .enumerate()
            .flat_map(|(take, t)| {
                segments(t).into_iter().map(move |features| Template {
                    label: t.label,
                    take,
                    features,
                })
            })
            .collect();
        Self { templates }
    }

    #[must_use]
    pub fn classify(&self, features: &[f32]) -> Option<Label> {
        self.nearest(features, None)
    }

    fn nearest(&self, features: &[f32], excluded_take: Option<usize>) -> Option<Label> {
        self.templates
            .iter()
            .filter(|t| Some(t.take) != excluded_take)
            .map(|t| (squared_distance(&t.features, features), t.label))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, label)| label)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn sample(t: u64, g: [f32; 3]) -> TimedSample {
        TimedSample {
            t,
            a: [0.0, 0.0, 1.0],
            g,
        }
    }

    pub fn swing(label: Label, axis: usize, sign: f32) -> Take {
        let samples = (0..150u16)
            .map(|i| {
                let bump = 300.0 * (-((f32::from(i) - 75.0) / 10.0).powi(2)).exp();
                let mut g = [0.0; 3];
                if let Some(v) = g.get_mut(axis) {
                    *v = sign * bump;
                }
                sample(u64::from(i) * 10, g)
            })
            .collect();
        Take {
            label,
            start_ms: 0,
            samples,
            optical: Vec::new(),
        }
    }

    pub fn knock() -> Take {
        let samples = (0..150u16)
            .map(|i| TimedSample {
                t: u64::from(i) * 10,
                a: [0.0, 0.0, if i == 75 { 3.0 } else { 1.0 }],
                g: [0.0; 3],
            })
            .collect();
        Take {
            label: Label::Knock,
            start_ms: 0,
            samples,
            optical: Vec::new(),
        }
    }

    #[test]
    fn plateau_counts_as_one_peak_at_its_first_sample() {
        let magnitudes = [0.0, 100.0, 100.0, 0.0];
        let peaks: Vec<usize> = (0..4).filter(|&i| is_peak(&magnitudes, i, 50.0)).collect();
        assert_eq!(peaks, vec![1]);
    }

    #[test]
    fn features_pad_with_edge_samples_near_the_start() {
        let samples: Vec<TimedSample> = (0..200u16)
            .map(|i| sample(u64::from(i), [200.0 * f32::from(i), 0.0, 0.0]))
            .collect();
        let f = features(&samples, 10);
        assert_eq!(
            (f.len(), f.first().copied(), f.get(60 * 6).copied()),
            (720, Some(0.0), Some(10.0))
        );
    }

    #[test]
    fn classifies_by_nearest_template() {
        let recognizer =
            Recognizer::train(&[swing(Label::Left, 1, 1.0), swing(Label::Right, 1, -1.0)]);
        let query = segments(&swing(Label::Idle, 1, -0.9));
        let labels: Vec<Option<Label>> = query.iter().map(|f| recognizer.classify(f)).collect();
        assert_eq!(labels, vec![Some(Label::Right)]);
    }
}
