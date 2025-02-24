use clap::Parser;
use serde_json::json;
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;
use zenoh::{config::WhatAmI, key_expr::OwnedKeyExpr, Config};

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Copy)]
pub enum LabelSetting {
    Index,
    Label,
    Score,
    LabelScore,
    Track,
}

#[derive(Debug, Clone, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// path to the mcap file
    #[arg(env, required = true)]
    pub mcap: PathBuf,

    /// replay speed
    #[arg(short, long, env, default_value = "1.0")]
    pub replay_speed: f64,

    /// raw dma topic
    #[arg(long, default_value = "rt/camera/dma")]
    pub dma_topic: String,

    /// list all topics
    #[arg(short, long)]
    pub list: bool,

    /// topics to publish. If empty, will publish all topics
    #[arg(short, long, env, value_delimiter = ' ', value_parser = parse_topics)]
    pub topics: Vec<Option<OwnedKeyExpr>>,

    /// topics to ignore
    #[arg(short, long, env, required = false, value_delimiter = ' ', value_parser = parse_topics)]
    pub ignore_topics: Vec<Option<OwnedKeyExpr>>,

    /// Application log level
    #[arg(long, env, default_value = "info")]
    pub rust_log: LevelFilter,

    /// Enable Tracy profiler broadcast
    #[arg(long, env)]
    pub tracy: bool,

    /// zenoh connection mode
    #[arg(long, env, default_value = "peer")]
    mode: WhatAmI,

    /// connect to zenoh endpoints
    #[arg(long, env)]
    connect: Vec<String>,

    /// listen to zenoh endpoints
    #[arg(long, env)]
    listen: Vec<String>,

    /// disable zenoh multicast scouting
    #[arg(long, env)]
    no_multicast_scouting: bool,
}

// Parse into Ok(None) when the topic string is empty. This covers the edge case
// of TOPICS="". Later this will be filtered out with `remove_none`
fn parse_topics(topics: &str) -> Result<Option<OwnedKeyExpr>, String> {
    if topics.is_empty() {
        return Ok(None);
    }
    let mut _topics = topics.to_owned();
    if _topics.starts_with("/") {
        _topics = "rt".to_owned() + &_topics;
    }
    match OwnedKeyExpr::autocanonize(_topics) {
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
