pub mod evaluate;

use std::sync::Arc;

use crate::dataset::{Label, Take};
use crate::timeline::TimedSample;

const PRE_SAMPLES: usize = 25;
pub const POST_SAMPLES: usize = 22;
pub const EARLY_MIN_POST: usize = 1;
const EARLY_DISTANCE_RATIO: f32 = 0.94;
const EARLY_NEIGHBORS: usize = 3;
const FULL_IDLE_RATIO: f32 = 0.95;
pub const RISING_WINDOW: usize = 16;
const RISING_MAX_LEAD: usize = 25;
const RISING_MIN_ACTIVITY: f32 = 55.0;
const RISING_HISTORY: usize = 12;
const RISING_MIN_GAIN: f32 = 20.0;
const RISING_RECENT_SPAN: usize = 6;
const RISING_MIN_RETAINED: f32 = 0.70;
const RISING_IDLE_STRIDE: usize = 2;
const RISING_GESTURE_NEIGHBORS: usize = 3;
const RISING_IDLE_NEIGHBORS: usize = 2;
const RISING_DISTANCE_RATIO: f32 = 0.90;
pub const PEAK_ACTIVITY: f32 = 60.0;
const GYRO_SCALE_DPS: f32 = 200.0;
const DPS_PER_G_OF_JOLT: f32 = 50.0;
const MAX_NEIGHBORS: usize = 3;
const LABEL_COUNT: usize = Label::ALL.len();

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
        .get(at.saturating_sub(PRE_SAMPLES)..at)
        .unwrap_or_default();
    let after = magnitudes
        .get(at + 1..magnitudes.len().min(at + 1 + POST_SAMPLES))
        .unwrap_or_default();
    peak >= threshold && before.iter().all(|&m| m < peak) && after.iter().all(|&m| m <= peak)
}

#[must_use]
pub fn is_rising_tail(magnitudes: &[f32], end: usize) -> bool {
    if end < RISING_HISTORY {
        return false;
    }
    let Some(&current) = magnitudes.get(end) else {
        return false;
    };
    if current < RISING_MIN_ACTIVITY {
        return false;
    }
    let baseline = magnitudes
        .get(end - RISING_HISTORY..end)
        .unwrap_or_default()
        .iter()
        .copied()
        .fold(f32::INFINITY, f32::min);
    if current <= baseline + RISING_MIN_GAIN {
        return false;
    }
    let recent_peak = magnitudes
        .get(end - RISING_RECENT_SPAN..=end)
        .unwrap_or_default()
        .iter()
        .copied()
        .fold(0.0, f32::max);
    current >= recent_peak * RISING_MIN_RETAINED
}

fn encode(sample: &TimedSample) -> [f32; 6] {
    let [gx, gy, gz] = sample.g.map(|v| v / GYRO_SCALE_DPS);
    [gx, gy, gz, sample.a[0], sample.a[1], sample.a[2]]
}

#[must_use]
pub fn features(samples: &[TimedSample], centre: usize, post: usize) -> Vec<f32> {
    let last = samples.len().saturating_sub(1);
    (0..=PRE_SAMPLES + post)
        .map(|k| (centre + k).saturating_sub(PRE_SAMPLES).min(last))
        .filter_map(|i| samples.get(i))
        .flat_map(encode)
        .collect()
}

#[must_use]
pub fn tail_features(samples: &[TimedSample], end: usize) -> Vec<f32> {
    let last = samples.len().saturating_sub(1);
    let span = RISING_WINDOW - 1;
    (0..RISING_WINDOW)
        .map(|k| (end + k).saturating_sub(span).min(last))
        .filter_map(|i| samples.get(i))
        .flat_map(encode)
        .collect()
}

pub fn monotonic(samples: &[TimedSample]) -> Vec<TimedSample> {
    let mut sorted: Vec<TimedSample> = samples.iter().rev().copied().collect();
    sorted.sort_by_key(|s| s.t);
    sorted.dedup_by_key(|s| s.t);
    sorted
}

fn peak_index(magnitudes: &[f32]) -> Option<usize> {
    magnitudes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
}

#[must_use]
pub fn segments(take: &Take) -> Vec<Vec<f32>> {
    let samples = monotonic(&take.samples);
    let magnitudes: Vec<f32> = samples.iter().map(activity).collect();
    let centres: Vec<usize> = match take.label {
        Label::Idle => (0..samples.len())
            .filter(|&i| is_peak(&magnitudes, i, PEAK_ACTIVITY))
            .collect(),
        Label::Twist
        | Label::Left
        | Label::Right
        | Label::Up
        | Label::Down
        | Label::ReverseTwist
        | Label::Knock => peak_index(&magnitudes).into_iter().collect(),
    };
    centres
        .into_iter()
        .map(|c| features(&samples, c, POST_SAMPLES))
        .collect()
}

const fn label_index(label: Label) -> usize {
    match label {
        Label::Idle => 0,
        Label::Twist => 1,
        Label::Left => 2,
        Label::Right => 3,
        Label::Up => 4,
        Label::Down => 5,
        Label::ReverseTwist => 6,
        Label::Knock => 7,
    }
}

struct Template {
    label: Label,
    take: usize,
    features: Vec<f32>,
}

struct RisingGroup {
    label: Label,
    take: usize,
    windows: Vec<Vec<f32>>,
}

struct TopK {
    slots: [f32; MAX_NEIGHBORS],
    limit: usize,
}

impl TopK {
    const fn new(limit: usize) -> Self {
        Self {
            slots: [f32::INFINITY; MAX_NEIGHBORS],
            limit,
        }
    }

    fn insert(&mut self, distance: f32) {
        let Some(last) = self.limit.checked_sub(1) else {
            return;
        };
        let Some(tail) = self.slots.get_mut(last) else {
            return;
        };
        if distance >= *tail {
            return;
        }
        *tail = distance;
        let mut i = last;
        while i > 0 {
            let (Some(&prev), Some(&curr)) = (self.slots.get(i - 1), self.slots.get(i)) else {
                break;
            };
            if curr < prev {
                self.slots.swap(i, i - 1);
                i -= 1;
            } else {
                break;
            }
        }
    }

    fn mean(&self) -> f32 {
        let mut sum = 0.0;
        let mut count = 0u16;
        for &value in self.slots.iter().take(self.limit) {
            if value.is_finite() {
                sum += value;
                count += 1;
            }
        }
        if count == 0 {
            f32::INFINITY
        } else {
            sum / f32::from(count)
        }
    }
}

#[derive(Clone)]
pub struct Recognizer {
    templates: Arc<[Template]>,
    rising_gestures: Arc<[RisingGroup]>,
    rising_idle: Arc<[Template]>,
    excluded_take: Option<usize>,
}

fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn pick_with_margin(scores: &[f32; LABEL_COUNT], ratio: f32) -> Option<Label> {
    let (best_idx, &best_score) = scores
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))?;
    if !best_score.is_finite() {
        return None;
    }
    let label = *Label::ALL.get(best_idx)?;
    if label == Label::Idle {
        return None;
    }
    let rival = scores
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != best_idx)
        .map(|(_, &d)| d)
        .fold(f32::INFINITY, f32::min);
    (best_score < rival * ratio * ratio).then_some(label)
}

impl Recognizer {
    #[must_use]
    pub fn train<'a>(takes: impl IntoIterator<Item = &'a Take>) -> Self {
        let mut templates = Vec::new();
        let mut rising_gestures = Vec::new();
        let mut rising_idle = Vec::new();
        for (take, t) in takes.into_iter().enumerate() {
            for feature_vec in segments(t) {
                templates.push(Template {
                    label: t.label,
                    take,
                    features: feature_vec,
                });
            }
            let samples = monotonic(&t.samples);
            let magnitudes: Vec<f32> = samples.iter().map(activity).collect();
            match t.label {
                Label::Idle => {
                    let mut i = RISING_WINDOW.max(RISING_HISTORY);
                    while i < samples.len() {
                        if is_rising_tail(&magnitudes, i) {
                            rising_idle.push(Template {
                                label: Label::Idle,
                                take,
                                features: tail_features(&samples, i),
                            });
                            i += RISING_IDLE_STRIDE;
                        } else {
                            i += 1;
                        }
                    }
                }
                Label::Twist
                | Label::Left
                | Label::Right
                | Label::Up
                | Label::Down
                | Label::ReverseTwist
                | Label::Knock => {
                    if let Some(peak) = peak_index(&magnitudes) {
                        let windows: Vec<Vec<f32>> = (0..=RISING_MAX_LEAD)
                            .filter_map(|lead| peak.checked_sub(lead))
                            .filter(|&end| is_rising_tail(&magnitudes, end))
                            .map(|end| tail_features(&samples, end))
                            .collect();
                        if !windows.is_empty() {
                            rising_gestures.push(RisingGroup {
                                label: t.label,
                                take,
                                windows,
                            });
                        }
                    }
                }
            }
        }
        Self {
            templates: Arc::from(templates),
            rising_gestures: Arc::from(rising_gestures),
            rising_idle: Arc::from(rising_idle),
            excluded_take: None,
        }
    }

    #[must_use]
    pub fn holding_out(&self, take: usize) -> Self {
        Self {
            templates: Arc::clone(&self.templates),
            rising_gestures: Arc::clone(&self.rising_gestures),
            rising_idle: Arc::clone(&self.rising_idle),
            excluded_take: Some(take),
        }
    }

    #[must_use]
    pub fn classify(&self, features: &[f32]) -> Option<Label> {
        self.nearest(features, self.excluded_take)
            .map(|(_, label)| label)
    }

    #[must_use]
    pub fn classify_early(&self, prefix: &[f32]) -> Option<Label> {
        let scores = self.peak_scores(prefix, EARLY_NEIGHBORS, self.excluded_take);
        pick_with_margin(&scores, EARLY_DISTANCE_RATIO)
    }

    #[must_use]
    pub fn classify_rising(&self, tail: &[f32]) -> Option<Label> {
        let mut best: [TopK; LABEL_COUNT] = std::array::from_fn(|i| {
            let k = if i == label_index(Label::Idle) {
                RISING_IDLE_NEIGHBORS
            } else {
                RISING_GESTURE_NEIGHBORS
            };
            TopK::new(k)
        });
        for group in self.rising_gestures.iter() {
            if Some(group.take) == self.excluded_take {
                continue;
            }
            let closest = group
                .windows
                .iter()
                .map(|w| squared_distance(w, tail))
                .fold(f32::INFINITY, f32::min);
            if let Some(bucket) = best.get_mut(label_index(group.label)) {
                bucket.insert(closest);
            }
        }
        for template in self.rising_idle.iter() {
            if Some(template.take) == self.excluded_take {
                continue;
            }
            let distance = squared_distance(&template.features, tail);
            if let Some(bucket) = best.get_mut(label_index(Label::Idle)) {
                bucket.insert(distance);
            }
        }
        let scores: [f32; LABEL_COUNT] =
            std::array::from_fn(|i| best.get(i).map_or(f32::INFINITY, TopK::mean));
        pick_with_margin(&scores, RISING_DISTANCE_RATIO)
    }

    fn peak_scores(
        &self,
        features: &[f32],
        neighbors: usize,
        excluded_take: Option<usize>,
    ) -> [f32; LABEL_COUNT] {
        let mut best: [TopK; LABEL_COUNT] = std::array::from_fn(|_| TopK::new(neighbors));
        for template in self.templates.iter() {
            if Some(template.take) == excluded_take {
                continue;
            }
            let distance = squared_distance(&template.features, features);
            if let Some(bucket) = best.get_mut(label_index(template.label)) {
                bucket.insert(distance);
            }
        }
        std::array::from_fn(|i| best.get(i).map_or(f32::INFINITY, TopK::mean))
    }

    fn nearest(&self, features: &[f32], excluded_take: Option<usize>) -> Option<(f32, Label)> {
        let scores = self.peak_scores(features, 1, excluded_take);
        let (best_idx, &best_score) = scores
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))?;
        if !best_score.is_finite() {
            return None;
        }
        let label = *Label::ALL.get(best_idx)?;
        if label != Label::Idle {
            let idle_score = scores
                .get(label_index(Label::Idle))
                .copied()
                .unwrap_or(f32::INFINITY);
            if best_score >= idle_score * FULL_IDLE_RATIO * FULL_IDLE_RATIO {
                return Some((idle_score, Label::Idle));
            }
        }
        Some((best_score, label))
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
        let f = features(&samples, 10, POST_SAMPLES);
        assert_eq!(
            (f.len(), f.first().copied(), f.get(PRE_SAMPLES * 6).copied()),
            ((PRE_SAMPLES + 1 + POST_SAMPLES) * 6, Some(0.0), Some(10.0))
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

    fn early(recognizer: &Recognizer, take: &Take) -> Option<Label> {
        recognizer.classify_early(&features(&take.samples, 75, EARLY_MIN_POST))
    }

    fn rivals() -> Recognizer {
        Recognizer::train(&[
            swing(Label::Left, 1, 1.0),
            swing(Label::Right, 1, -1.0),
            swing(Label::Idle, 0, 1.0),
        ])
    }

    #[test]
    fn a_prefix_far_closer_to_one_gesture_classifies_early() {
        assert_eq!(
            early(&rivals(), &swing(Label::Left, 1, 0.95)),
            Some(Label::Left)
        );
    }

    #[test]
    fn a_prefix_nearest_to_idle_does_not_classify_early() {
        assert_eq!(early(&rivals(), &swing(Label::Idle, 0, 0.95)), None);
    }

    #[test]
    fn a_prefix_between_two_gestures_does_not_classify_early() {
        assert_eq!(early(&rivals(), &swing(Label::Left, 1, 0.05)), None);
    }

    #[test]
    fn a_clear_rising_edge_classifies_before_the_peak() {
        let query = tail_features(&swing(Label::Left, 1, 0.95).samples, 69);
        assert_eq!(rivals().classify_rising(&query), Some(Label::Left));
    }

    #[test]
    fn a_flat_high_activity_tail_is_not_a_rising_edge() {
        let flat = vec![120.0; 30];
        assert!(!is_rising_tail(&flat, 29));
    }
}
