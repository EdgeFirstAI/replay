// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! EdgeFirst MCAP replay service.

mod args;
mod image_publish;
mod services;
mod video_decode;

use args::Args;
use clap::Parser;
use edgefirst_hal::tensor::TensorDyn;
#[allow(deprecated)]
use edgefirst_schemas::edgefirst_msgs::DmaBuffer;
use edgefirst_schemas::{
    builtin_interfaces::Time, foxglove_msgs::FoxgloveCompressedVideo, sensor_msgs::CompressedImage,
};
use image_publish::HalImagePublisher;
use log::{debug, error, info, warn};
use mcap::Message;
use memmap2::Mmap;
use services::ServiceHandler;
use std::thread::sleep;
use std::{
    collections::HashSet,
    error::Error,
    fs,
    os::fd::AsRawFd,
    path::Path,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::{info_span, instrument};
use tracing_subscriber::{layer::SubscriberExt as _, Layer as _, Registry};
use tracy_client::{frame_mark, secondary_frame_mark};
use video_decode::{JpegStream, VideoDecoder};
use videostream::frame::Frame;
use zenoh::{
    bytes::{Encoding, ZBytes},
    key_expr::{KeyExpr, OwnedKeyExpr},
    Session, Wait,
};

const DMA_SCHEMA: &str = "edgefirst_msgs/msg/DmaBuffer";
const NV12_FOURCC: u32 = u32::from_le_bytes(*b"NV12");

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

const INIT_TIME_VAL: u64 = 0;

fn main() {
    let args = Args::parse();

    let _tracy = args.tracy.then(tracy_client::Client::start);

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

    let topics: Vec<OwnedKeyExpr> = args.topics.iter().flatten().cloned().collect();
    let ignore_topics: Vec<OwnedKeyExpr> = args.ignore_topics.iter().flatten().cloned().collect();

    // Hal-backed RGBA image publisher. Lives across replay-loop restarts;
    // its pre-allocated destination ring and inode-keyed source cache are
    // never invalidated. Disabled when --camera-image-topic is empty.
    let mut hal_publisher = if args.camera_image_topic.is_empty() {
        None
    } else {
        Some(HalImagePublisher::new(
            args.camera_image_topic.clone(),
            args.camera_image_buffers,
        ))
    };

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
            service_handler.stop_services(&topics_to_publish);
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

        let mut video_decoder: Option<VideoDecoder> = None;
        let mut jpeg_stream: Option<JpegStream> = None;

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
                sleep(dur);
            }

            let schema = match &message.channel.schema {
                Some(v) => v.name.clone(),
                None => "".to_string(),
            };

            if schema == "edgefirst_msgs/msg/DmaBuffer" {
                // Don't re-publish recorded DMA buffer messages — the fd
                // references in the MCAP belong to the original publisher's
                // process and are meaningless here.
                continue;
            }

            if schema == "foxglove_msgs/msg/CompressedVideo" {
                has_h264 = true;
                stream_h264(
                    &message,
                    &mut video_decoder,
                    src_pid,
                    &args,
                    &session,
                    hal_publisher.as_mut(),
                );
                args.tracy.then(|| secondary_frame_mark!("h264"));
            }

            // we don't use jpeg for DMA buffer when h264 is present
            if !has_h264 && schema == "sensor_msgs/msg/CompressedImage" {
                stream_jpeg(
                    &message,
                    &mut jpeg_stream,
                    src_pid,
                    &args,
                    &session,
                    hal_publisher.as_mut(),
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
fn stream_h264(
    message: &Message,
    video_decoder: &mut Option<VideoDecoder>,
    src_pid: u32,
    args: &Args,
    session: &Session,
    hal_publisher: Option<&mut HalImagePublisher>,
) {
    let video = match FoxgloveCompressedVideo::<&[u8]>::from_cdr(&message.data) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not deserialize CompressedVideo message: {:?}", e);
            return;
        }
    };
    if video.format() != "h264" {
        error!("Unsupported CompressedVideo format {}", video.format());
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

    let frame = match video_decoder.decode_h264_msg(video.data()) {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };

    let stamp = video.stamp();
    let frame_id = video.frame_id();

    if let Err(e) = publish_frame_dma(&frame, stamp, frame_id, src_pid, &args.dma_topic, session) {
        error!("Failed to publish dma message: {:?}", e);
    }

    if let Some(publisher) = hal_publisher {
        let (vw, vh) = match video_decoder.crop() {
            Ok(c) => (c.width() as u32, c.height() as u32),
            Err(e) => {
                warn!("hal publish skipped — decoder crop unavailable: {:?}", e);
                return;
            }
        };
        if let Err(e) = publisher.publish_from_frame(&frame, vw, vh, stamp, frame_id, session) {
            warn!("hal image publish failed: {:?}", e);
        }
    }
}

#[instrument(skip_all)]
fn stream_jpeg(
    message: &Message,
    jpeg_stream: &mut Option<JpegStream>,
    src_pid: u32,
    args: &Args,
    session: &Session,
    hal_publisher: Option<&mut HalImagePublisher>,
) {
    let image = match CompressedImage::<&[u8]>::from_cdr(&message.data) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not deserialize CompressedImage message: {:?}", e);
            return;
        }
    };
    if image.format() != "jpeg" {
        error!("Unsupported CompressedImage format {}", image.format());
        return;
    }

    if jpeg_stream.is_none() {
        match JpegStream::new() {
            Ok(v) => jpeg_stream.insert(v),
            Err(e) => {
                error!("Could not open jpeg stream: {:?}", e);
                return;
            }
        };
    }
    let jpeg_stream = jpeg_stream.as_mut().unwrap();

    let tensor = match jpeg_stream.decode(image.data()) {
        Ok(t) => t,
        Err(e) => {
            error!("Could not decode jpeg message: {:?}", e);
            return;
        }
    };

    let stamp = image.stamp();
    let frame_id = image.frame_id();

    if let Err(e) = publish_tensor_dma(tensor, stamp, frame_id, src_pid, &args.dma_topic, session) {
        error!("Failed to publish dma message: {:?}", e);
    }

    if let Some(publisher) = hal_publisher {
        let vw = tensor.width().unwrap_or(0) as u32;
        let vh = tensor.height().unwrap_or(0) as u32;
        if let Err(e) = publisher.publish_from_tensor(tensor, vw, vh, stamp, frame_id, session) {
            warn!("hal image publish failed: {:?}", e);
        }
    }
}

/// Publish a videostream Frame as a `DmaBuffer` carrying decoder-native NV12.
fn publish_frame_dma(
    frame: &Frame,
    stamp: Time,
    frame_id: &str,
    pid: u32,
    topic: &str,
    session: &Session,
) -> Result<(), Box<dyn Error>> {
    let fd = frame.handle()?;
    let width = frame.width()? as u32;
    let height = frame.height()? as u32;
    let stride = frame.stride()? as u32;
    let fourcc = frame.fourcc()?;
    // NV12 buffer length: stride * height * 3 / 2. With `stride == width`
    // (the common case) this is the natural NV12 size; with non-tight stride
    // the receiver still gets a contiguous buffer to mmap.
    let length = (stride as u64 * height as u64 * 3 / 2) as u32;

    publish_dma_buffer(
        stamp, frame_id, pid, fd, width, height, stride, fourcc, length, topic, session,
    )
}

/// Publish a hal NV12 dma-buf TensorDyn as a `DmaBuffer`.
fn publish_tensor_dma(
    tensor: &TensorDyn,
    stamp: Time,
    frame_id: &str,
    pid: u32,
    topic: &str,
    session: &Session,
) -> Result<(), Box<dyn Error>> {
    let fd_borrow = tensor.dmabuf()?;
    let fd = fd_borrow.as_raw_fd();
    let width = tensor.width().ok_or("tensor missing width")? as u32;
    let height = tensor.height().ok_or("tensor missing height")? as u32;
    let stride = tensor
        .effective_row_stride()
        .map(|s| s as u32)
        .unwrap_or(width);
    let length = (stride as u64 * height as u64 * 3 / 2) as u32;

    publish_dma_buffer(
        stamp,
        frame_id,
        pid,
        fd,
        width,
        height,
        stride,
        NV12_FOURCC,
        length,
        topic,
        session,
    )
}

#[allow(clippy::too_many_arguments, deprecated)]
fn publish_dma_buffer(
    stamp: Time,
    frame_id: &str,
    pid: u32,
    fd: i32,
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    length: u32,
    topic: &str,
    session: &Session,
) -> Result<(), Box<dyn Error>> {
    let msg = DmaBuffer::new(
        stamp, frame_id, pid, fd, width, height, stride, fourcc, length,
    )?;
    let bytes = msg.into_cdr();
    let enc = Encoding::APPLICATION_CDR.with_schema(DMA_SCHEMA);
    session
        .put(topic, ZBytes::from(bytes))
        .encoding(enc)
        .wait()
        .map_err(|e| format!("zenoh put on {topic} failed: {e:?}"))?;
    debug!(
        "Sent dma message on {topic} fd={fd} {width}x{height} stride={stride} \
         fourcc=0x{fourcc:08x} length={length}"
    );
    Ok(())
}
