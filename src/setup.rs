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
    pub topics: Vec<OwnedKeyExpr>,

    /// topics to ignore
    #[arg(short, long, env, required = false, value_delimiter = ' ', value_parser = parse_topics)]
    pub ignore_topics: Vec<OwnedKeyExpr>,
}

fn parse_topics(topics: &str) -> Result<OwnedKeyExpr, &'static str> {
    if topics.is_empty() {
        return Err("Topic cannot be empty string");
    }
    let mut topics = topics.to_owned();
    if topics.starts_with("/") {
        topics = "rt".to_owned() + &topics;
    }
    match OwnedKeyExpr::autocanonize(topics) {
        Ok(v) => Ok(v),
        Err(_) => Err("Could not parse topic"),
    }
}
