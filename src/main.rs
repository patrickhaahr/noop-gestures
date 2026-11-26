mod calibrator;
mod dataset;
mod frame;
mod imu;
mod probe;
mod strap;
mod timeline;

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
    }
}
