// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! CLI argument parsing and Zenoh configuration.

use clap::Parser;
use serde_json::json;
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;
use zenoh::{config::WhatAmI, key_expr::OwnedKeyExpr, Config};

/// Command-line arguments for EdgeFirst Replay Node.
///
/// This structure defines all configuration options for the replay node,
/// including MCAP file selection, playback control, Zenoh configuration,
/// and debugging options. Arguments can be specified via command line or
/// environment variables.
///
/// # Example
///
/// ```bash
/// # Via command line
/// edgefirst-replay recording.mcap --replay-speed 2.0
///
/// # Via environment variables
/// export MCAP=/path/to/recording.mcap
/// export REPLAY_SPEED=2.0
/// edgefirst-replay
/// ```
#[derive(Debug, Clone, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the MCAP recording file to replay
    #[arg(env = "MCAP", required = true)]
    pub mcap: PathBuf,

    /// Replay speed multiplier (must be greater than 0)
    #[arg(short, long, env = "REPLAY_SPEED", default_value = "1.0", value_parser = parse_replay_speed)]
    pub replay_speed: f64,

    /// Zenoh topic for raw DMA buffer metadata
    #[arg(long, default_value = "rt/camera/dma")]
    pub dma_topic: String,

    /// List all topics in the MCAP file and exit
    #[arg(short, long)]
    pub list: bool,

    /// Replay the MCAP file only once (no looping)
    #[arg(short, long)]
    pub one_shot: bool,

    /// Stop system services before replay
    #[arg(short, long)]
    pub system: bool,

    /// Zenoh topics to publish (space-delimited; empty = publish all)
    #[arg(short, long, env = "TOPICS", value_delimiter = ' ', value_parser = parse_topics)]
    pub topics: Vec<Option<OwnedKeyExpr>>,

    /// Zenoh topics to ignore during replay (space-delimited)
    #[arg(short, long, env = "IGNORE_TOPICS", required = false, value_delimiter = ' ', value_parser = parse_topics)]
    pub ignore_topics: Vec<Option<OwnedKeyExpr>>,

    /// Application log level
    #[arg(long, env = "RUST_LOG", default_value = "info")]
    pub rust_log: LevelFilter,

    /// Enable Tracy profiler broadcast
    #[arg(long, env = "TRACY")]
    pub tracy: bool,

    /// Zenoh participant mode (peer, client, or router)
    #[arg(long, env = "MODE", default_value = "peer")]
    mode: WhatAmI,

    /// Zenoh endpoints to connect to (can specify multiple)
    #[arg(long, env = "CONNECT")]
    connect: Vec<String>,

    /// Zenoh endpoints to listen on (can specify multiple)
    #[arg(long, env = "LISTEN")]
    listen: Vec<String>,

    /// Disable Zenoh multicast peer discovery
    #[arg(long, env = "NO_MULTICAST_SCOUTING")]
    no_multicast_scouting: bool,
}

fn parse_replay_speed(s: &str) -> Result<f64, String> {
    let speed: f64 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if speed <= 0.0 {
        return Err("replay speed must be greater than 0".to_string());
    }
    if !speed.is_finite() {
        return Err("replay speed must be a finite number".to_string());
    }
    Ok(speed)
}

// Parse into Ok(None) when the topic string is empty. This covers the edge case
// of TOPICS="". Later this will be filtered out with `remove_none`
fn parse_topics(topics: &str) -> Result<Option<OwnedKeyExpr>, String> {
    if topics.is_empty() {
        return Ok(None);
    }
    let mut topic = topics.to_owned();
    if topic.starts_with("/") {
        topic = "rt".to_owned() + &topic;
    }
    match OwnedKeyExpr::autocanonize(topic) {
        Ok(v) => Ok(Some(v)),
        Err(_) => Err(format!("Could not parse topic: {topics}")),
    }
}

impl From<Args> for Config {
    fn from(args: Args) -> Self {
        let mut config = Config::default();

        config
            .insert_json5("mode", &json!(args.mode).to_string())
            .unwrap();

        if !args.connect.is_empty() {
            config
                .insert_json5("connect/endpoints", &json!(args.connect).to_string())
                .unwrap();
        }

        if !args.listen.is_empty() {
            config
                .insert_json5("listen/endpoints", &json!(args.listen).to_string())
                .unwrap();
        }

        if args.no_multicast_scouting {
            config
                .insert_json5("scouting/multicast/enabled", &json!(false).to_string())
                .unwrap();
        }

        config
            .insert_json5("scouting/multicast/interface", &json!("lo").to_string())
            .unwrap();

        config
    }
}
