// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! Hal-backed RGBA publisher for `rt/camera/image`.
//!
//! Converts decoder-native NV12 frames (h264) or hal-decoded NV12 tensors
//! (jpeg) to RGBA using `edgefirst_hal::image::ImageProcessor` and publishes
//! as `sensor_msgs/Image`. Enabled via `--camera-image-topic`.

use edgefirst_hal::image::{Crop, Flip, ImageProcessor, ImageProcessorTrait, Rect, Rotation};
use edgefirst_hal::tensor::{DType, PixelFormat, TensorDyn, TensorMapTrait, TensorTrait};
use edgefirst_schemas::{builtin_interfaces::Time, sensor_msgs::Image};
use log::{debug, error, info};
use nix::sys::stat::fstat;
use std::{
    collections::{hash_map::Entry, HashMap},
    error::Error,
    os::fd::BorrowedFd,
};
use tracing::instrument;
use videostream::frame::Frame;
use zenoh::{
    bytes::{Encoding, ZBytes},
    Session, Wait,
};

const ROS_IMAGE_SCHEMA: &str = "sensor_msgs/msg/Image";
const RGBA_ENCODING: &str = "rgba8";

/// Hal-backed RGBA image publisher.
///
/// Owns a pre-allocated RGBA destination ring (never freed) and an
/// inode-keyed source-tensor cache (populated lazily as new pool slots
/// appear, never freed during a decoder session). The processor is created
/// once and reused.
pub struct HalImagePublisher {
    topic: String,
    ring_size: usize,
    cdr_scratch: Vec<u8>,
    state: Option<Ready>,
}

struct Ready {
    processor: ImageProcessor,
    dst_ring: Vec<TensorDyn>,
    src_cache: HashMap<u64, TensorDyn>,
    next_dst: usize,
    visible_width: u32,
    visible_height: u32,
    /// Scratch buffer used only when the destination tensor is allocated
    /// with row-stride padding (i.e. effective_row_stride > width*4). For
    /// GPU-pre-aligned widths (e.g. 1920) hal returns a tight buffer and
    /// this stays empty.
    rgba_pack: Vec<u8>,
}

impl HalImagePublisher {
    pub fn new(topic: String, ring_size: usize) -> Self {
        Self {
            topic,
            ring_size: ring_size.max(1),
            cdr_scratch: Vec::new(),
            state: None,
        }
    }

    /// Convert a videostream NV12 Frame to RGBA and publish.
    ///
    /// `visible_width`/`visible_height` come from `Decoder::crop()` and pin
    /// the destination size on first call; subsequent calls must use the
    /// same values for the publisher's lifetime.
    #[instrument(skip_all)]
    pub fn publish_from_frame(
        &mut self,
        frame: &Frame,
        visible_width: u32,
        visible_height: u32,
        stamp: Time,
        frame_id: &str,
        session: &Session,
    ) -> Result<(), Box<dyn Error>> {
        let fd = frame.handle()?;
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let ino = fstat(borrowed)?.st_ino;

        let frame_w = frame.width()? as usize;
        let frame_h = frame.height()? as usize;
        let frame_stride = frame.stride()? as usize;
        let frame_fourcc = frame.fourcc()?;

        // Split borrows so `ensure_ready` can take `&mut self.state` while
        // `self.topic` / `self.cdr_scratch` are still independently available.
        let Self {
            topic,
            ring_size,
            cdr_scratch,
            state,
        } = self;
        let ready = ensure_ready(state, *ring_size, topic, visible_width, visible_height)?;

        if let Entry::Vacant(slot) = ready.src_cache.entry(ino) {
            let owned = borrowed.try_clone_to_owned()?;
            let format = fourcc_to_pixel_format(frame_fourcc)?;
            let shape = tensor_shape_for(format, frame_w, frame_h)?;
            let mut src = TensorDyn::from_fd(owned, &shape, DType::U8, Some("replay-h264-src"))?;
            src.set_format(format)?;
            if frame_stride > frame_w {
                src.set_row_stride(frame_stride)?;
            }
            debug!(
                "hal src cache insert ino={} {}x{} stride={} format={:?}",
                ino, frame_w, frame_h, frame_stride, format
            );
            slot.insert(src);
        }

        let src_rect = Rect::new(0, 0, visible_width as usize, visible_height as usize);
        convert_and_publish(
            ready,
            ino,
            Some(src_rect),
            stamp,
            frame_id,
            topic,
            session,
            cdr_scratch,
        )
    }

    /// Convert an NV12 hal tensor (e.g. jpeg-decoded) to RGBA and publish.
    #[instrument(skip_all)]
    pub fn publish_from_tensor(
        &mut self,
        src: &TensorDyn,
        visible_width: u32,
        visible_height: u32,
        stamp: Time,
        frame_id: &str,
        session: &Session,
    ) -> Result<(), Box<dyn Error>> {
        let borrowed = src.dmabuf()?;
        let ino = fstat(borrowed)?.st_ino;
        let width = src.width().ok_or("tensor missing width")?;
        let height = src.height().ok_or("tensor missing height")?;
        let format = src.format().ok_or("tensor missing format")?;

        let Self {
            topic,
            ring_size,
            cdr_scratch,
            state,
        } = self;
        let ready = ensure_ready(state, *ring_size, topic, visible_width, visible_height)?;

        if let Entry::Vacant(slot) = ready.src_cache.entry(ino) {
            let owned = borrowed.try_clone_to_owned()?;
            let shape = tensor_shape_for(format, width, height)?;
            let mut tensor = TensorDyn::from_fd(owned, &shape, DType::U8, Some("replay-jpeg-src"))?;
            tensor.set_format(format)?;
            if let Some(stride) = src.effective_row_stride() {
                if stride > width {
                    tensor.set_row_stride(stride)?;
                }
            }
            debug!(
                "hal src cache insert (tensor) ino={} {}x{} format={:?}",
                ino, width, height, format
            );
            slot.insert(tensor);
        }

        let src_rect = Rect::new(0, 0, visible_width as usize, visible_height as usize);
        convert_and_publish(
            ready,
            ino,
            Some(src_rect),
            stamp,
            frame_id,
            topic,
            session,
            cdr_scratch,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn convert_and_publish(
    ready: &mut Ready,
    src_key: u64,
    src_rect: Option<Rect>,
    stamp: Time,
    frame_id: &str,
    topic: &str,
    session: &Session,
    cdr_scratch: &mut Vec<u8>,
) -> Result<(), Box<dyn Error>> {
    let dst_idx = ready.next_dst;
    ready.next_dst = (ready.next_dst + 1) % ready.dst_ring.len();

    let src = ready
        .src_cache
        .get(&src_key)
        .expect("src cache entry just inserted or already present");
    let dst = &mut ready.dst_ring[dst_idx];

    let crop = Crop::new().with_src_rect(src_rect);
    ready
        .processor
        .convert(src, dst, Rotation::None, Flip::None, crop)?;

    let width = ready.visible_width;
    let height = ready.visible_height;
    let step = width * 4;

    // Fast path: if the destination tensor has natural row stride
    // (`width * 4`), the mmap is already a tightly-packed RGBA frame and we
    // can pass it directly to the CDR builder. Hal's `create_image` returns
    // natural-stride buffers for the GPU-pre-aligned widths (640, 1280,
    // 1920, 3008, 3840). When the allocator pads, fall back to a row-by-row
    // copy into a scratch buffer kept inside `Ready`.
    let row_bytes = (width as usize) * 4;
    let stride = dst.effective_row_stride().unwrap_or(row_bytes);
    let tensor_u8 = dst
        .as_u8()
        .ok_or("hal destination tensor is not u8-backed")?;
    let map = tensor_u8.map()?;
    let src = map.as_slice();
    let height_usize = height as usize;

    let enc = Encoding::APPLICATION_CDR.with_schema(ROS_IMAGE_SCHEMA);
    if stride == row_bytes {
        let needed = row_bytes * height_usize;
        Image::builder()
            .stamp(stamp)
            .frame_id(frame_id)
            .height(height)
            .width(width)
            .encoding(RGBA_ENCODING)
            .step(step)
            .data(&src[..needed])
            .encode_into_vec(cdr_scratch)?;
    } else {
        ready.rgba_pack.resize(row_bytes * height_usize, 0);
        for row in 0..height_usize {
            let s = row * stride;
            let e = s + row_bytes;
            ready.rgba_pack[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(&src[s..e]);
        }
        Image::builder()
            .stamp(stamp)
            .frame_id(frame_id)
            .height(height)
            .width(width)
            .encoding(RGBA_ENCODING)
            .step(step)
            .data(&ready.rgba_pack)
            .encode_into_vec(cdr_scratch)?;
    }
    drop(map);

    session
        .put(topic, ZBytes::from(cdr_scratch.as_slice()))
        .encoding(enc)
        .wait()
        .map_err(|e| format!("zenoh put failed: {e:?}"))?;
    Ok(())
}

fn ensure_ready<'a>(
    state: &'a mut Option<Ready>,
    ring_size: usize,
    topic: &str,
    visible_width: u32,
    visible_height: u32,
) -> Result<&'a mut Ready, Box<dyn Error>> {
    if state.is_none() {
        info!(
            "Initialising hal image publisher: {}x{} ring={} topic={}",
            visible_width, visible_height, ring_size, topic
        );
        let processor = ImageProcessor::new()?;
        let mut dst_ring = Vec::with_capacity(ring_size);
        for _ in 0..ring_size {
            let t = processor.create_image(
                visible_width as usize,
                visible_height as usize,
                PixelFormat::Rgba,
                DType::U8,
                None,
            )?;
            dst_ring.push(t);
        }
        *state = Some(Ready {
            processor,
            dst_ring,
            src_cache: HashMap::new(),
            next_dst: 0,
            visible_width,
            visible_height,
            rgba_pack: Vec::new(),
        });
    }
    let ready = state.as_mut().expect("just initialised");
    if ready.visible_width != visible_width || ready.visible_height != visible_height {
        error!(
            "hal publisher dims changed {}x{} -> {}x{}; ignoring",
            ready.visible_width, ready.visible_height, visible_width, visible_height
        );
    }
    Ok(ready)
}

fn fourcc_to_pixel_format(fourcc: u32) -> Result<PixelFormat, Box<dyn Error>> {
    let bytes = fourcc.to_le_bytes();
    match &bytes {
        b"NV12" => Ok(PixelFormat::Nv12),
        b"YUYV" => Ok(PixelFormat::Yuyv),
        _ => Err(format!(
            "unsupported source fourcc {:?} for hal image path",
            String::from_utf8_lossy(&bytes)
        )
        .into()),
    }
}

fn tensor_shape_for(
    format: PixelFormat,
    width: usize,
    height: usize,
) -> Result<Vec<usize>, Box<dyn Error>> {
    match format {
        PixelFormat::Nv12 => {
            if !height.is_multiple_of(2) {
                return Err(format!("NV12 requires even height, got {height}").into());
            }
            Ok(vec![height * 3 / 2, width])
        }
        PixelFormat::Yuyv => Ok(vec![height, width, 2]),
        other => Err(format!("unsupported pixel format {other:?}").into()),
    }
}
