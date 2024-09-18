use clap::Parser;
use std::path::PathBuf;

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
    #[arg(required = true)]
    pub mcap: PathBuf,

    /// replay speed
    #[arg(short, long, default_value = "1.0")]
    pub replay_speed: f64,

    /// raw dma topic
    #[arg(long, default_value = "rt/camera/dma")]
    pub dma_topic: String,

    /// list all topics.
    #[arg(short, long)]
    pub list: bool,

    /// echo out all topics.
    #[arg(short, long)]
    pub all_topics: bool,

    /// when --all_topics is not enabled, this is the list of topics to echo
    /// out. When --all_topics is enabled, filters out these topics instead
    #[arg(env, required = false, value_delimiter = ' ')]
    pub topic_filter: Vec<String>,
}
