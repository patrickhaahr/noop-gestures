use std::collections::BTreeMap;
use std::fmt;

use super::{Recognizer, segments};
use crate::dataset::{Label, Take};

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
