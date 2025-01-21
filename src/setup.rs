use clap::Parser;
use std::path::PathBuf;
use zenoh::prelude::OwnedKeyExpr;

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
    /// connect to zenoh endpoints
    #[arg(long, default_value = "tcp/127.0.0.1:7447")]
    pub connect: Vec<String>,

    /// listen to zenoh endpoints
    #[arg(long)]
    pub listen: Vec<String>,

    /// zenoh connection mode
    #[arg(long, default_value = "client")]
    pub mode: String,

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
