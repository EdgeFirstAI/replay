// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

mod args;
mod image;
mod services;
mod video_decode;

use args::Args;
use cdr::{CdrLe, Infinite};
use clap::Parser;
use edgefirst_schemas::{
    edgefirst_msgs::DmaBuf, foxglove_msgs::FoxgloveCompressedVideo, sensor_msgs::CompressedImage,
    std_msgs::Header,
};
use image::{Image, ImageManager};
use log::{error, info, trace};
use mcap::Message;
use memmap2::Mmap;
use services::ServiceHandler;
use std::{
    collections::HashSet,
    fs,
    path::Path,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tracing::{info_span, instrument};
use tracing_subscriber::{layer::SubscriberExt as _, Layer as _, Registry};
use tracy_client::{frame_mark, secondary_frame_mark};
use video_decode::VideoDecoder;
use zenoh::{
    bytes::{Encoding, ZBytes},
    key_expr::{KeyExpr, OwnedKeyExpr},
    Session, Wait,
};

fn map_mcap<P: AsRef<Path>>(p: P) -> Result<Mmap, String> {
    let fd = match fs::File::open(p.as_ref()) {
        Ok(v) => v,
        Err(e) => return Err(format!("Couldn't open MCAP file: {:#?} {e}", p.as_ref())),
    };
    match unsafe { Mmap::map(&fd) } {
        Ok(v) => Ok(v),
        Err(e) => Err(format!("Couldn't map MCAP file: {e}")),
    }
}

fn get_topics(mapped: &Mmap) -> HashSet<String> {
    let mut topics = HashSet::new();

    if let Ok(Some(summary)) = mcap::Summary::read(mapped) {
        for c in summary.channels.values() {
            let topic = c.topic.clone();
            topics.insert(topic);
        }

        if !topics.is_empty() {
            return topics;
        }
    }
    // Didn't find topics in summary, proceed to find topics by looping
    // through all the messages
    let msg_stream = match mcap::MessageStream::new(mapped) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not parse mcap file: {:?}", e);
            return topics;
        }
    };
    for message in msg_stream {
        let message = match message {
            Ok(v) => v,
            Err(e) => {
                error!("Could not parse mcap message: {:?}", e);
                continue;
            }
        };
        let topic = message.channel.topic.clone();
        topics.insert(topic);
    }
    topics
}

fn filter_topic(
    include_topics: &[OwnedKeyExpr],
    ignore_topics: &[OwnedKeyExpr],
    mcap_topic: &str,
) -> bool {
    let topic = "rt".to_owned() + mcap_topic;
    let topic = KeyExpr::autocanonize(topic).unwrap_or_else(|_| {
        panic!("mcap topic {mcap_topic} cannot be converted to valid zenoh topic")
    });
    let mut to_publish = include_topics.is_empty();

    for t in include_topics {
        if t.includes(&topic) {
            to_publish = true;
            break;
        }
    }

    for t in ignore_topics {
        if t.includes(&topic) {
            to_publish = false;
            break;
        }
    }

    to_publish
}

pub fn remove_none(topics: Vec<Option<OwnedKeyExpr>>) -> Vec<OwnedKeyExpr> {
    topics.into_iter().flatten().collect()
}

const INIT_TIME_VAL: u64 = 0;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    args.tracy.then(tracy_client::Client::start);

    let stdout_log = tracing_subscriber::fmt::layer()
        .pretty()
        .with_filter(args.rust_log);

    let journald = match tracing_journald::layer() {
        Ok(journald) => Some(journald.with_filter(args.rust_log)),
        Err(_) => None,
    };

    let tracy = match args.tracy {
        true => Some(tracing_tracy::TracyLayer::default().with_filter(args.rust_log)),
        false => None,
    };

    let subscriber = Registry::default()
        .with(stdout_log)
        .with(journald)
        .with(tracy);
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    tracing_log::LogTracer::init().unwrap();

    let mapped = match map_mcap(&args.mcap) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not open mcap file: {:?}", e);
            return;
        }
    };
    info!("Opened MCAP file {:?}", args.mcap);

    if args.list {
        let topics = get_topics(&mapped);

        if topics.is_empty() {
            println!("Did not find any topics in MCAP");
            return;
        }
        for t in topics {
            println!("{}", t);
        }
        return;
    }

    let run = Arc::new(AtomicBool::new(true));
    let run_clone = run.clone();
    ctrlc::set_handler(move || {
        if !run_clone.fetch_and(false, Ordering::Relaxed) {
            process::exit(0);
        }
    })
    .expect("Error setting Ctrl-C handler");

    loop {
        let msg_stream = match mcap::MessageStream::new(&mapped) {
            Ok(v) => v,
            Err(e) => {
                error!("Could not parse mcap file: {:?}", e);
                return;
            }
        };
        info!("Parsed MCAP file {:?}", args.mcap);
        let src_pid = process::id();

        let mut has_h264 = false;

        let topics = remove_none(args.topics.clone());
        let ignore_topics = remove_none(args.ignore_topics.clone());

        info!("Publishing topics: {:?}", topics);
        info!("Ignoring topics: {:?}", ignore_topics);

        let topics_to_publish: HashSet<_> = get_topics(&mapped)
            .into_iter()
            .filter(|t| filter_topic(&topics, &ignore_topics, t))
            .collect();
        info!(
            "Found the following topics to publish: {:#?}",
            topics_to_publish
        );

        let service_handler = ServiceHandler::new();
        if args.system {
            info!("Stopping system services before replay");
            let services_stop = service_handler.stop_services(&topics_to_publish);
            let _ = services_stop.join_all().await;
        } else {
            info!("Keeping system services running");
        }

        let msg_stream = msg_stream.filter(|message| {
            let message = match message {
                Ok(v) => v,
                Err(e) => {
                    error!("Could not parse mcap message: {:?}", e);
                    return false;
                }
            };
            topics_to_publish.contains(&message.channel.topic)
        });

        let session = zenoh::open(args.clone()).wait().unwrap();

        let mut first_msg_time = INIT_TIME_VAL;
        let mut start = Instant::now();

        let imgmgr = match ImageManager::new() {
            Ok(v) => v,
            Err(e) => {
                error!("Could not open G2D: {:?}", e);
                return;
            }
        };

        info!("Opened G2D with version {}", imgmgr.version());

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
                );
                args.tracy.then(|| secondary_frame_mark!("h264"));
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
                );
                args.tracy.then(|| secondary_frame_mark!("jpeg"));
            }

            info_span!("publish").in_scope(|| {
                let msg = ZBytes::from(message.data.as_ref());
                let enc = Encoding::APPLICATION_CDR.with_schema(schema.clone());

                match session
                    .put("rt".to_string() + &message.channel.topic, msg)
                    .encoding(enc)
                    .wait()
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
            });

            args.tracy.then(frame_mark);
        }

        if args.one_shot {
            break;
        }
        info!("Replay finished, starting over...");
    }
}

#[instrument(skip_all)]
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
    let frame = match video_decoder.decode_h264_msg(&video.data, imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };

    if let Some(f) = frame {
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
        let dma_msg = build_dma_msg_image(f, video.header.clone(), src_pid, args);
        let msg = ZBytes::from(cdr::serialize::<_, _, CdrLe>(&dma_msg, Infinite).unwrap());
        let enc = Encoding::APPLICATION_CDR.with_schema("edgefirst_msgs/msg/DmaBuffer");

        match session.put(&args.dma_topic, msg).encoding(enc).wait() {
            Ok(_) => trace!("Sent dma message on {}", args.dma_topic),
            Err(e) => {
                error!("Error sending message on {}: {:?}", args.dma_topic, e)
            }
        }
    }
}

#[instrument(skip_all)]
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
    let frame = match video_decoder.decode_jpeg_msg(&image.data, imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };
    if let Some(f) = frame {
        let dma_msg = build_dma_msg_image(f, image.header.clone(), src_pid, args);
        let msg = ZBytes::from(cdr::serialize::<_, _, CdrLe>(&dma_msg, Infinite).unwrap());
        let enc = Encoding::APPLICATION_CDR.with_schema("edgefirst_msgs/msg/DmaBuffer");

        match session.put(&args.dma_topic, msg).encoding(enc).wait() {
            Ok(_) => (),
            Err(e) => {
                error!("Error sending message on {}: {:?}", args.dma_topic, e)
            }
        }
    }
}

fn build_dma_msg_image(buf: &Image, header: Header, pid: u32, args: &Args) -> DmaBuf {
    let _ = args;

    // let ts = buf.timestamp();
    let width = buf.width();
    let height = buf.height();
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
