use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::optical::OpticalSample;
use crate::timeline::TimedSample;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Idle,
    Overview,
    Left,
    Right,
    WorkspaceUp,
    WorkspaceDown,
    ReverseTwist,
    Knock,
    Fist,
    Pinch,
}

impl Label {
    pub const ALL: [Self; 10] = [
        Self::Idle,
        Self::Overview,
        Self::Left,
        Self::Right,
        Self::WorkspaceUp,
        Self::WorkspaceDown,
        Self::ReverseTwist,
        Self::Knock,
        Self::Fist,
        Self::Pinch,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Overview => "overview",
            Self::Left => "left",
            Self::Right => "right",
            Self::WorkspaceUp => "workspace_up",
            Self::WorkspaceDown => "workspace_down",
            Self::ReverseTwist => "reverse_twist",
            Self::Knock => "knock",
            Self::Fist => "fist",
            Self::Pinch => "pinch",
        }
    }

    #[must_use]
    pub const fn window_ms(self) -> u64 {
        match self {
            Self::Idle => 6_000,
            Self::Overview
            | Self::Left
            | Self::Right
            | Self::WorkspaceUp
            | Self::WorkspaceDown
            | Self::ReverseTwist
            | Self::Knock
            | Self::Fist
            | Self::Pinch => 1_500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    pub label: Label,
    pub start_ms: u64,
    pub samples: Vec<TimedSample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optical: Vec<OpticalSample>,
}

pub struct Dataset {
    path: PathBuf,
}

impl Dataset {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, take: &Take) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(take)?)
    }

    pub fn takes(&self) -> io::Result<Vec<Take>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(io::Error::from))
            .collect()
    }

    pub fn remove_last(&self) -> io::Result<Option<Take>> {
        let mut takes = self.takes()?;
        let removed = takes.pop();
        let body: String = takes
            .iter()
            .map(serde_json::to_string)
            .map(|line| line.map(|l| l + "\n"))
            .collect::<Result<_, _>>()?;
        fs::write(&self.path, body)?;
        Ok(removed)
    }

    pub fn counts(&self) -> io::Result<BTreeMap<Label, usize>> {
        let mut counts: BTreeMap<Label, usize> = Label::ALL.iter().map(|&l| (l, 0)).collect();
        for take in self.takes()? {
            *counts.entry(take.label).or_default() += 1;
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Dataset {
        let path = std::env::temp_dir()
            .join(format!("noop-gestures-{name}-{}", std::process::id()))
            .join("takes.jsonl");
        fs::remove_file(&path).ok();
        Dataset::new(path)
    }

    fn take(label: Label) -> Take {
        Take {
            label,
            start_ms: 1,
            samples: vec![TimedSample {
                t: 1,
                a: [0.0, 0.0, 1.0],
                g: [0.0; 3],
            }],
            optical: Vec::new(),
        }
    }

    #[test]
    fn appended_takes_round_trip() -> io::Result<()> {
        let dataset = scratch("round-trip");
        dataset.append(&take(Label::Left))?;
        assert_eq!(dataset.takes()?, vec![take(Label::Left)]);
        Ok(())
    }

    #[test]
    fn counts_every_label_including_empty_ones() -> io::Result<()> {
        let dataset = scratch("counts");
        dataset.append(&take(Label::Right))?;
        dataset.append(&take(Label::Right))?;
        let counts = dataset.counts()?;
        assert_eq!(
            (counts.get(&Label::Right), counts.get(&Label::Idle)),
            (Some(&2), Some(&0))
        );
        Ok(())
    }

    #[test]
    fn remove_last_drops_only_the_newest_take() -> io::Result<()> {
        let dataset = scratch("remove-last");
        dataset.append(&take(Label::Left))?;
        dataset.append(&take(Label::Overview))?;
        let removed = dataset.remove_last()?.map(|t| t.label);
        assert_eq!(
            (removed, dataset.takes()?),
            (Some(Label::Overview), vec![take(Label::Left)])
        );
        Ok(())
    }
}
