use async_std::task::sleep;
use cdr::{CdrLe, Infinite};
// use serde::{Deserialize, Serialize};
use clap::Parser;
use ctrlc;
use edgefirst_schemas::{
    edgefirst_msgs::DmaBuf, foxglove_msgs::FoxgloveCompressedVideo, sensor_msgs::CompressedImage,
    std_msgs::Header,
};
use image::{Image, ImageManager};
use log::{error, info, trace};
use mcap::Message;
use setup::Args;
use std::{
    collections::HashSet,
    path::Path,
    process,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use video_decode::VideoDecoder;
use zenoh::{
    config::Config,
    prelude::{r#async::*, sync::SyncResolve},
};
mod image;
mod setup;
mod video_decode;

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

// Sanitize the topics by removing preceding "rt" from any "rt/[...]" topics.
// Removes empty string topics. Then sorts the topics
fn sanitize_topics(topics: &mut Vec<String>) {
    topics.retain_mut(|x| {
        if x.starts_with("rt/") {
            *x = x[2..].to_string();
        }
        !x.is_empty()
    });
    topics.sort();
}

const INIT_TIME_VAL: u64 = 0;

#[async_std::main]
async fn main() {
    let args = Args::parse();
    if args.replay_speed <= 0.0 {
        println!("replay_speed must be a positive number")
    }
    env_logger::init();

    let mapped = match map_mcap(&args.mcap) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not open mcap file: {:?}", e);
            return;
        }
    };
    info!("Opened MCAP file {:?}", args.mcap);
    let msg_stream = match mcap::MessageStream::new(&mapped) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not parse mcap file: {:?}", e);
            return;
        }
    };
    info!("Parsed MCAP file {:?}", args.mcap);

    if args.list {
        let mut topics = HashSet::new();

        let summary = mcap::Summary::read(&mapped);
        if summary.is_ok() && summary.as_ref().unwrap().is_some() {
            let summary = summary.unwrap().unwrap();
            for c in summary.channels.values() {
                let topic = c.topic.clone();
                if !topics.contains(&topic) {
                    println!("{topic}");
                }
                topics.insert(topic);
            }
            // Didn't find topics in summary, proceed to find topics by looping
            // through all the messages
            if !topics.is_empty() {
                return;
            }
        }

        for message in msg_stream {
            let message = match message {
                Ok(v) => v,
                Err(e) => {
                    error!("Could not parse mcap message: {:?}", e);
                    return;
                }
            };
            let topic = message.channel.topic.clone();
            if !topics.contains(&topic) {
                println!("{topic}");
            }
            topics.insert(topic);
        }
        if topics.is_empty() {
            println!("Did not find any topics in MCAP");
        }
        return;
    }

    let src_pid = process::id();

    let mut has_h264 = false;

    let mut topics = args.topics.clone();
    let mut ignore_topics = args.ignore_topics.clone();
    sanitize_topics(&mut topics);
    sanitize_topics(&mut ignore_topics);

    info!("Publishing topics: {:?}", topics);
    info!("Ignoring topics: {:?}", ignore_topics);
    let msg_stream = msg_stream.filter(|message| {
        let message = match message {
            Ok(v) => v,
            Err(e) => {
                error!("Could not parse mcap message: {:?}", e);
                return false;
            }
        };
        let to_publish = if topics.is_empty() {
            true
        } else {
            topics.binary_search(&message.channel.topic).is_ok()
        };

        let to_ignore = ignore_topics.binary_search(&message.channel.topic).is_ok();

        to_publish && !to_ignore
    });
    let mut config = Config::default();
    let mode = WhatAmI::from_str(&args.mode).unwrap();
    config.set_mode(Some(mode)).unwrap();
    config.connect.endpoints = args.connect.iter().map(|v| v.parse().unwrap()).collect();
    config.listen.endpoints = args.listen.iter().map(|v| v.parse().unwrap()).collect();
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

    let mut first_msg_time = INIT_TIME_VAL;
    let mut start = Instant::now();

    let run = Arc::new(AtomicBool::new(true));
    let run_clone = run.clone();
    ctrlc::set_handler(move || {
        if !run_clone.fetch_and(false, Ordering::Relaxed) {
            process::exit(0);
        }
    })
    .expect("Error setting Ctrl-C handler");

    let imgmgr = match ImageManager::new() {
        Ok(v) => v,
        Err(e) => {
            error!("Could not open G2D: {:?}", e);
            return;
        }
    };

    let mut video_decoder = None;

    for message in msg_stream {
        if !run.load(Ordering::Relaxed) {
            return;
        }
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
                ((message.log_time - first_msg_time) as f64 / args.replay_speed) as u64,
            )
            .checked_sub(start.elapsed())
            .unwrap_or_default();
            sleep(dur).await
        }
        let schema = match &message.channel.schema {
            Some(v) => v.name.clone(),
            None => "".to_string(),
        };

        if schema == "edgefirst_msgs/msg/DmaBuffer" {
            // Don't send DMA buffer messages because they won't be useful
            continue;
        }

        if schema == "foxglove_msgs/msg/CompressedVideo" {
            has_h264 = true;
            stream_h264(
                &message,
                &mut video_decoder,
                &imgmgr,
                src_pid,
                &args,
                &session,
            )
        }

        // we don't use jpeg for DMA buffer when h264 is present
        if !has_h264 && schema == "sensor_msgs/msg/CompressedImage" {
            stream_jpeg(
                &message,
                &mut video_decoder,
                &imgmgr,
                src_pid,
                &args,
                &session,
            )
        }

        let value = Value::from(message.data.as_ref());
        let value = value.encoding(Encoding::WithSuffix(
            KnownEncoding::AppOctetStream,
            schema.clone().into(),
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
            }
        }
    }
}

fn stream_h264<'a>(
    message: &Message,
    video_decoder: &mut Option<VideoDecoder<'a>>,
    imgmgr: &'a ImageManager,
    src_pid: u32,
    args: &Args,
    session: &Session,
) {
    let video: FoxgloveCompressedVideo = match cdr::deserialize(&message.data) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not deserialize CompressedVideo message: {:?}", e);
            return;
        }
    };
    if video.format != "h264" {
        error!("Unsupported CompressedVideo format {}", video.format);
        return;
    }

    if video_decoder.is_none() {
        match VideoDecoder::new() {
            Ok(v) => video_decoder.insert(v),
            Err(e) => {
                error!("Could not open video decoder: {:?}", e);
                return;
            }
        };
    }
    let video_decoder = video_decoder.as_mut().unwrap();
    // let count = video_decoder.frame_count;
    let frame = match video_decoder.decode_h264_msg(&video.data, &imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };

    match frame {
        Some(f) => {
            // use std::{fs::File, io::Write};
            // let _ = f.dmabuf().memory_map().unwrap().read(
            //     move |b, _: Option<i32>| {
            //         let mut file = File::create(format!("./frame{}.rgba", count))
            //             .expect("Unable to create file");
            //         file.write(b)?;
            //         Ok(())
            //     },
            //     Some(1),
            // );
            let dma_msg = build_dma_msg_image(f, video.header.clone(), src_pid, &args);
            let encoded = Value::from(cdr::serialize::<_, _, CdrLe>(&dma_msg, Infinite).unwrap())
                .encoding(Encoding::WithSuffix(
                    KnownEncoding::AppOctetStream,
                    "edgefirst_msgs/msg/DmaBuffer".into(),
                ));

            match session.put(&args.dma_topic, encoded).res_sync() {
                Ok(_) => trace!("Sent dma message on {}", args.dma_topic),
                Err(e) => {
                    error!("Error sending message on {}: {:?}", args.dma_topic, e)
                }
            }
        }
        None => {}
    }
}

fn stream_jpeg<'a>(
    message: &Message,
    video_decoder: &mut Option<VideoDecoder<'a>>,
    imgmgr: &'a ImageManager,
    src_pid: u32,
    args: &Args,
    session: &Session,
) {
    let image: CompressedImage = match cdr::deserialize(&message.data) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not deserialize CompressedImage message: {:?}", e);
            return;
        }
    };
    if image.format != "jpeg" {
        error!("Unsupported CompressedImage format {}", image.format);
        return;
    }

    if video_decoder.is_none() {
        match VideoDecoder::new() {
            Ok(v) => video_decoder.insert(v),
            Err(e) => {
                error!("Could not open video decoder: {:?}", e);
                return;
            }
        };
    }
    let video_decoder = video_decoder.as_mut().unwrap();
    let frame = match video_decoder.decode_jpeg_msg(&image.data, &imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };
    match frame {
        Some(f) => {
            let dma_msg = build_dma_msg_image(f, image.header.clone(), src_pid, &args);
            let encoded = Value::from(cdr::serialize::<_, _, CdrLe>(&dma_msg, Infinite).unwrap())
                .encoding(Encoding::WithSuffix(
                    KnownEncoding::AppOctetStream,
                    "edgefirst_msgs/msg/DmaBuffer".into(),
                ));

            match session.put(&args.dma_topic, encoded).res_sync() {
                Ok(_) => (),
                Err(e) => {
                    error!("Error sending message on {}: {:?}", args.dma_topic, e)
                }
            }
        }
        None => {}
    }
}

fn build_dma_msg_image(buf: &Image, header: Header, pid: u32, args: &Args) -> DmaBuf {
    let _ = args;

    // let ts = buf.timestamp();
    let width = buf.width() as u32;
    let height = buf.height() as u32;
    let fourcc = buf.format().into();
    let dma_buf = buf.raw_fd();
    // let dma_buf = buf.original_fd;
    let length = buf.size() as u32;
    let msg = DmaBuf {
        header,
        pid,
        fd: dma_buf,
        width,
        height,
        stride: width,
        fourcc,
        length,
    };
    trace!(
        "dmabuf dma_buf: {} pid: {} length: {}",
        dma_buf,
        pid,
        length,
    );
    msg
}
