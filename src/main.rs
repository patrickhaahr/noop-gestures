mod calibrator;
mod daemon;
mod dataset;
mod detector;
mod frame;
mod imu;
mod optical;
mod probe;
mod recognizer;
mod session;
mod strap;
mod timeline;

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

use crate::dataset::Dataset;

#[derive(Parser)]
#[command(about = "Turn WHOOP 4.0 wrist motion into desktop gestures")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Print live IMU batch statistics from the strap")]
    Probe {
        #[arg(default_value_t = 15)]
        seconds: u64,
        #[arg(long, help = "Also send SET_IMU_DATA_STREAM (command 106)")]
        imu_stream: bool,
    },
    #[command(about = "Serve the gesture calibrator on 127.0.0.1 and record labelled takes")]
    Calibrate {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        #[arg(long, help = "JSONL file the takes are appended to")]
        dataset: Option<PathBuf>,
    },
    #[command(about = "Recognize gestures from the live strap and drive niri")]
    Run {
        #[arg(long, help = "JSONL file of recorded takes")]
        dataset: Option<PathBuf>,
    },
    #[command(about = "Score the recognizer on the recorded takes, holding out one take at a time")]
    Evaluate {
        #[arg(long, help = "JSONL file of recorded takes")]
        dataset: Option<PathBuf>,
    },
}

fn default_dataset() -> Result<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .context("neither XDG_DATA_HOME nor HOME is set")?;
    Ok(data_home.join("noop-gestures/takes.jsonl"))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Probe {
            seconds,
            imu_stream,
        } => probe::run(seconds, imu_stream).await,
        Cmd::Calibrate { port, dataset } => {
            let path = dataset.map_or_else(default_dataset, Ok)?;
            calibrator::run(port, Dataset::new(path)).await
        }
        Cmd::Run { dataset } => {
            let path = dataset.map_or_else(default_dataset, Ok)?;
            daemon::run(&Dataset::new(path)).await
        }
        Cmd::Evaluate { dataset } => {
            let path = dataset.map_or_else(default_dataset, Ok)?;
            let takes = Dataset::new(path).takes()?;
            let confusion = recognizer::evaluate::leave_one_take_out(&takes);
            writeln!(std::io::stdout(), "{confusion}")?;
            Ok(())
        }
    }
}
