use std::collections::BTreeMap;
use std::fmt;

use super::{Recognizer, monotonic, segments};
use crate::dataset::{Label, Take};
use crate::detector::Detector;
use crate::timeline::NOMINAL_SAMPLE_PERIOD_US;

const LONG_IDLE_SAMPLES: usize = 6_000;
const BATCH_SAMPLES: usize = 100;

pub struct Replay {
    seconds: u64,
    fired: BTreeMap<Label, usize>,
}

#[must_use]
pub fn replay_long_idle(takes: &[Take]) -> Vec<Replay> {
    takes
        .iter()
        .enumerate()
        .filter(|(_, t)| t.label == Label::Idle && t.samples.len() >= LONG_IDLE_SAMPLES)
        .map(|(held_out, take)| {
            let others = takes.iter().enumerate().filter(|&(i, _)| i != held_out);
            let mut detector = Detector::new(Recognizer::train(others.map(|(_, t)| t)));
            let samples = monotonic(&take.samples);
            let mut fired = BTreeMap::new();
            for detection in samples
                .chunks(BATCH_SAMPLES)
                .flat_map(|batch| detector.push(batch))
            {
                *fired.entry(detection.label).or_default() += 1;
            }
            Replay {
                seconds: u64::try_from(samples.len()).unwrap_or(0) * NOMINAL_SAMPLE_PERIOD_US
                    / 1_000_000,
                fired,
            }
        })
        .collect()
}

impl fmt::Display for Replay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total: usize = self.fired.values().sum();
        let detail: Vec<String> = self
            .fired
            .iter()
            .map(|(label, n)| format!("{} {n}", label.name()))
            .collect();
        write!(
            f,
            "replayed a {} min {} s idle take live: {total} gestures would fire",
            self.seconds / 60,
            self.seconds % 60
        )?;
        if detail.is_empty() {
            Ok(())
        } else {
            write!(f, " ({})", detail.join(", "))
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Confusion(BTreeMap<(Label, Label), usize>);

impl Confusion {
    fn count(&self, actual: Label, predicted: Label) -> usize {
        self.0
            .get(&(actual, predicted))
            .copied()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.0.values().sum()
    }

    #[must_use]
    pub fn correct(&self) -> usize {
        Label::ALL.iter().map(|&l| self.count(l, l)).sum()
    }

    #[must_use]
    pub fn false_triggers(&self) -> usize {
        Label::ALL
            .iter()
            .filter(|&&l| l != Label::Idle)
            .map(|&l| self.count(Label::Idle, l))
            .sum()
    }
}

#[must_use]
pub fn leave_one_take_out(takes: &[Take]) -> Confusion {
    let recognizer = Recognizer::train(takes);
    let mut confusion = Confusion::default();
    for (index, take) in takes.iter().enumerate() {
        for features in segments(take) {
            if let Some(predicted) = recognizer.nearest(&features, Some(index)) {
                *confusion.0.entry((take.label, predicted)).or_default() += 1;
            }
        }
    }
    confusion
}

impl fmt::Display for Confusion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:>15}", "actual")?;
        for predicted in Label::ALL {
            write!(f, "{:>15}", predicted.name())?;
        }
        writeln!(f)?;
        for actual in Label::ALL {
            write!(f, "{:>15}", actual.name())?;
            for predicted in Label::ALL {
                write!(f, "{:>15}", self.count(actual, predicted))?;
            }
            writeln!(f)?;
        }
        write!(
            f,
            "{}/{} segments correct, {} idle segments would trigger a gesture",
            self.correct(),
            self.total(),
            self.false_triggers()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizer::tests::swing;

    #[test]
    fn held_out_take_is_never_its_own_neighbour() {
        let takes = [
            swing(Label::Left, 1, 1.0),
            swing(Label::Left, 1, 1.1),
            swing(Label::Right, 1, -1.0),
        ];
        let confusion = leave_one_take_out(&takes);
        assert_eq!(
            (
                confusion.count(Label::Left, Label::Left),
                confusion.count(Label::Right, Label::Left)
            ),
            (2, 1)
        );
    }
}
