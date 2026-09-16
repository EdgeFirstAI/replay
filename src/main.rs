mod args;
mod image;
mod services;
mod video_decode;

use args::Args;
use clap::Parser;
use edgefirst_schemas::{
    builtin_interfaces::Time,
    edgefirst_msgs::{CameraFrame, TensorFields, TensorPlaneView},
    foxglove_msgs::FoxgloveCompressedVideo,
    sensor_msgs::CompressedImage,
};
use image::{Image, ImageManager};
use log::{debug, error, info, warn};
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
const SCHEMA_DMA_BUFFER: &str = "edgefirst_msgs/msg/DmaBuffer";
const SCHEMA_CAMERA_FRAME: &str = "edgefirst_msgs/msg/CameraFrame";
/// DMA-backed tensor storage. The schema carries the value without interpreting it.
const TENSOR_STORAGE_DMA: u32 = 2;
const TENSOR_DTYPE_U8: u32 = 1;

struct FrameSink<'a> {
    src_pid: u32,
    frame_seq: &'a mut u64,
    frame_cdr: &'a mut Vec<u8>,
    args: &'a Args,
    session: &'a Session,
}

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

        // TODO: When we move to zenoh 1.0, update to use KeBoxTree instead of
        // Vec<KeyExpr>
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
            Ok(v) => {
                info!("Opened G2D with version {}", v.version());
                Some(v)
            }
            Err(e) => {
                warn!(
                    "Could not open G2D ({e:?}); compressed passthrough only, no CameraFrame synthesis"
                );
                None
            }
        };

        let mut video_decoder = None;
        let mut frame_cdr = Vec::new();
        let mut frame_seq = 0u64;

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

            if schema == SCHEMA_DMA_BUFFER || schema == SCHEMA_CAMERA_FRAME {
                // Recorded DMA/CameraFrame handles are process-local and stale.
                continue;
            }

            if schema == "foxglove_msgs/msg/CompressedVideo" {
                has_h264 = true;
                if let Some(imgmgr) = imgmgr.as_ref() {
                    stream_h264(
                        &message,
                        &mut video_decoder,
                        imgmgr,
                        &mut FrameSink {
                            src_pid,
                            frame_seq: &mut frame_seq,
                            frame_cdr: &mut frame_cdr,
                            args: &args,
                            session: &session,
                        },
                    );
                }
                args.tracy.then(|| secondary_frame_mark!("h264"));
            }

            // we don't use jpeg for DMA buffer when h264 is present
            if !has_h264 && schema == "sensor_msgs/msg/CompressedImage" {
                if let Some(imgmgr) = imgmgr.as_ref() {
                    stream_jpeg(
                        &message,
                        &mut video_decoder,
                        imgmgr,
                        &mut FrameSink {
                            src_pid,
                            frame_seq: &mut frame_seq,
                            frame_cdr: &mut frame_cdr,
                            args: &args,
                            session: &session,
                        },
                    );
                }
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
    sink: &mut FrameSink<'_>,
) {
    let video = match FoxgloveCompressedVideo::from_cdr(message.data.as_ref()) {
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
        *video_decoder = Some(VideoDecoder::new());
    }
    let video_decoder = video_decoder.as_mut().unwrap();
    let frame = match video_decoder.decode_h264_msg(video.data(), imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };

    if let Some(f) = frame {
        publish_camera_frame(f, video.stamp(), video.frame_id(), sink);
    }
}

#[instrument(skip_all)]
fn stream_jpeg<'a>(
    message: &Message,
    video_decoder: &mut Option<VideoDecoder<'a>>,
    imgmgr: &'a ImageManager,
    sink: &mut FrameSink<'_>,
) {
    let image = match CompressedImage::from_cdr(message.data.as_ref()) {
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

    if video_decoder.is_none() {
        *video_decoder = Some(VideoDecoder::new());
    }
    let video_decoder = video_decoder.as_mut().unwrap();
    let frame = match video_decoder.decode_jpeg_msg(image.data(), imgmgr) {
        Ok(v) => v,
        Err(e) => {
            error!("Could not decode video message: {:?}", e);
            return;
        }
    };
    if let Some(f) = frame {
        publish_camera_frame(f, image.stamp(), image.frame_id(), sink);
    }
}

fn publish_camera_frame(buf: &Image, stamp: Time, frame_id: &str, sink: &mut FrameSink<'_>) {
    let shape = [buf.height() as u64, buf.width() as u64];
    let length = buf.size() as u64;
    let planes = [TensorPlaneView {
        handle: buf.raw_fd() as i64,
        offset: 0,
        stride: buf.stride() as u64,
        size: length,
        used: length,
        modifier: 0,
        handle_bytes: &[],
        data: &[],
    }];
    let format = buf.format().to_string();
    let fields = TensorFields {
        storage_kind: TENSOR_STORAGE_DMA,
        pid: sink.src_pid,
        fence_fd: -1,
        dtype: TENSOR_DTYPE_U8,
        shape: &shape,
        planes: &planes,
        format: format.into(),
        ..Default::default()
    };

    *sink.frame_seq += 1;
    if let Err(e) = CameraFrame::builder()
        .stamp(stamp)
        .frame_id(frame_id)
        .seq(*sink.frame_seq)
        .tensor(&fields)
        .encode_into_vec(sink.frame_cdr)
    {
        error!("Could not encode CameraFrame: {:?}", e);
        return;
    }

    let msg = ZBytes::from(sink.frame_cdr.as_slice());
    let enc = Encoding::APPLICATION_CDR.with_schema(SCHEMA_CAMERA_FRAME);

    match sink
        .session
        .put(&sink.args.dma_topic, msg)
        .encoding(enc)
        .wait()
    {
        Ok(_) => debug!("Sent CameraFrame on {}", sink.args.dma_topic),
        Err(e) => {
            error!("Error sending message on {}: {:?}", sink.args.dma_topic, e)
        }
    }
}
