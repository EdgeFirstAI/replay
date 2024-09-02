use async_std::task::sleep;
// use serde::{Deserialize, Serialize};
use clap::Parser;
use log::{error, info};
use setup::Settings;
use std::{
    path::Path,
    str::FromStr,
    time::{Duration, Instant},
};
use zenoh::{
    config::Config,
    prelude::{r#async::*, sync::SyncResolve},
};
mod setup;

use memmap2::Mmap;
use std::fs;

fn map_mcap<P: AsRef<Path>>(p: P) -> Result<Mmap, String> {
    let fd = match fs::File::open(p.as_ref()) {
        Ok(v) => v,
        Err(e) => return Err(format!("Couldn't open MCAP file: {:#?} {e}", p.as_ref())),
    };
    match unsafe { Mmap::map(&fd) } {
        Ok(v) => Ok(v),
        Err(e) => return Err(format!("Couldn't map MCAP file: {e}")),
    }
}

const INIT_TIME_VAL: u64 = 0;

#[async_std::main]
async fn main() {
    let s = Settings::parse();
    if s.replay_speed <= 0.0 {
        println!("replay_speed must be a positive number")
    }
    env_logger::init();

    let mut config = Config::default();

    let mode = WhatAmI::from_str(&s.mode).unwrap();
    config.set_mode(Some(mode)).unwrap();
    config.connect.endpoints = s.connect.iter().map(|v| v.parse().unwrap()).collect();
    config.listen.endpoints = s.listen.iter().map(|v| v.parse().unwrap()).collect();
    let _ = config.scouting.multicast.set_enabled(Some(true));
    let _ = config
        .scouting
        .multicast
        .set_interface(Some("lo".to_string()));
    let _ = config.scouting.gossip.set_enabled(Some(true));
    let session = match zenoh::open(config.clone()).res_async().await {
        Ok(v) => v,
        Err(e) => {
            error!("Error while opening Zenoh session: {:?}", e);
            return;
        }
    }
    .into_arc();
    info!("Opened Zenoh session");

    let mapped = match map_mcap(&s.mcap) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not open mcap file: {:?}", e);
            return;
        }
    };

    let msg_stream = match mcap::MessageStream::new(&mapped) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not parse mcap file: {:?}", e);
            return;
        }
    };

    let mut first_msg_time = INIT_TIME_VAL;
    let mut start = Instant::now();
    for message in msg_stream {
        let message = match message {
            Ok(v) => v,
            Err(e) => {
                error!("Could not parse mcap message: {:?}", e);
                continue;
            }
        };

        if first_msg_time == INIT_TIME_VAL {
            start = Instant::now();
            first_msg_time = message.log_time;
        } else {
            let dur = Duration::from_nanos(
                ((message.log_time - first_msg_time) as f64 / s.replay_speed) as u64,
            )
            .checked_sub(start.elapsed())
            .unwrap_or_default();
            sleep(dur).await
        }

        if message.channel.message_encoding == "edgefirst_msgs/msg/DmaBuffer" {
            // Don't send DMA buffer messages because they won't be useful
            continue;
        }

        let value = Value::from(message.data.as_ref()).encoding(Encoding::WithSuffix(
            KnownEncoding::AppOctetStream,
            message.channel.message_encoding.clone().into(),
        ));
        match session
            .put("rt".to_string() + &message.channel.topic, value)
            .res_sync()
        {
            Ok(_) => (),
            Err(e) => {
                error!(
                    "Error sending message on {}: {:?}",
                    "rt".to_string() + &message.channel.topic,
                    e
                )
            } /* TODO: special condition h264 or jpeg streaming to allow dmabuf based services to
               * use them */
        }
    }
}
