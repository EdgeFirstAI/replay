// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! H.264 (VPU) and JPEG (hal codec) decoders for the replay pipeline.
//!
//! H.264 frames are surfaced directly as `videostream::Frame` — the caller
//! publishes the decoder-native NV12 buffer to `rt/camera/dma` and optionally
//! converts to RGBA via the hal `ImageProcessor` for `rt/camera/image`.
//!
//! JPEG frames are decoded by `edgefirst_codec::ImageDecoder` directly into
//! a pre-allocated NV12 dma-buf tensor ring — no host-side intermediate,
//! no memcpy.

use edgefirst_codec::{peek_info, DecodeOptions, ImageDecoder, ImageLoad};
use edgefirst_hal::image::ImageProcessor;
use edgefirst_hal::tensor::{DType, PixelFormat, TensorDyn};
use log::{info, trace, warn};
use std::{error::Error, thread::sleep, time::Duration};
use videostream::decoder::{CodecBackend, DecodeReturnCode, Decoder, DecoderCodec};
use videostream::encoder::VSLRect;
use videostream::frame::Frame;

const JPEG_RING_DEPTH: usize = 4;

pub struct VideoDecoder {
    decoder: Decoder,
    last_data: Vec<u8>,
    visible_logged: bool,
    pub frame_count: usize,
}

impl VideoDecoder {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        // Use the explicit Auto-backend constructor. The legacy `create()`
        // goes through the older `vsl_decoder_create` C entry which doesn't
        // run the v4l2 device enumeration the 2.5.x stack relies on; the
        // `_ex` entry does.
        Ok(VideoDecoder {
            decoder: Decoder::create_ex(DecoderCodec::H264, 30, CodecBackend::Auto)?,
            last_data: Vec::new(),
            visible_logged: false,
            frame_count: 0,
        })
    }

    /// Visible (post-crop) frame rectangle as reported by the H264 decoder.
    /// Only valid after the first frame has decoded.
    pub fn crop(&self) -> Result<VSLRect, Box<dyn Error>> {
        Ok(self.decoder.crop()?)
    }

    /// Decode one H264 message and return the next NV12 frame, if any.
    ///
    /// The returned `Frame` borrows a slot from the VPU pool — keep it alive
    /// across the publish call(s) so the receiver can still open its fd.
    ///
    /// `decode_frame` is transiently fallible: the V4L2 m2m OUTPUT queue
    /// can be momentarily full and the backend returns an `Io("Decoder Error")`
    /// until a slot frees up. We retry the same data on error, sleeping
    /// briefly between attempts, matching the camera-service replay pattern.
    pub fn decode_h264_msg(&mut self, data: &[u8]) -> Result<Option<Frame>, Box<dyn Error>> {
        const MAX_RETRIES: usize = 20;
        const RETRY_SLEEP: Duration = Duration::from_millis(5);

        self.last_data.extend_from_slice(data);
        let mut consumed = 0;
        let mut retries = 0;

        while consumed < self.last_data.len() {
            let slice = &self.last_data[consumed..];
            match self.decoder.decode_frame(slice) {
                Ok((ret, used, frame)) => {
                    consumed += used;
                    retries = 0;
                    trace!(
                        "decode_frame consumed {used} ({consumed}/{})",
                        self.last_data.len()
                    );

                    if ret == DecodeReturnCode::Initialized && !self.visible_logged {
                        let crop = self.decoder.crop()?;
                        info!("Video dimensions are: {}x{}", crop.width(), crop.height());
                        self.visible_logged = true;
                    }

                    if let Some(f) = frame {
                        self.frame_count += 1;
                        self.last_data.drain(..consumed);
                        return Ok(Some(f));
                    }

                    if used == 0 {
                        // No frame and no progress — need more input.
                        break;
                    }
                }
                Err(e) => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        warn!("Persistent decoder error after {retries} retries: {e:?}");
                        self.last_data.drain(..consumed);
                        return Err(Box::new(e));
                    }
                    sleep(RETRY_SLEEP);
                }
            }
        }

        self.last_data.drain(..consumed);
        Ok(None)
    }
}

/// Hal-backed JPEG stream: decodes JPEGs directly into a ring of NV12
/// dma-buf tensors so the published fd is the same one the codec wrote into.
pub struct JpegStream {
    processor: ImageProcessor,
    decoder: ImageDecoder,
    dst_ring: Vec<TensorDyn>,
    next_dst: usize,
}

impl JpegStream {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            processor: ImageProcessor::new()?,
            decoder: ImageDecoder::new(),
            dst_ring: Vec::new(),
            next_dst: 0,
        })
    }

    /// Decode one JPEG message into the next NV12 dma-buf tensor slot and
    /// return a reference to it.
    pub fn decode(&mut self, data: &[u8]) -> Result<&TensorDyn, Box<dyn Error>> {
        let opts = DecodeOptions::default().with_format(PixelFormat::Nv12);

        if self.dst_ring.is_empty() {
            let info = peek_info(data, &opts)?;
            info!(
                "JPEG dimensions are: {}x{} (decoding to NV12)",
                info.width, info.height
            );
            for _ in 0..JPEG_RING_DEPTH {
                let t = self.processor.create_image(
                    info.width,
                    info.height,
                    PixelFormat::Nv12,
                    DType::U8,
                    None,
                )?;
                self.dst_ring.push(t);
            }
        }

        let idx = self.next_dst;
        self.next_dst = (self.next_dst + 1) % self.dst_ring.len();
        self.dst_ring[idx].load_image(&mut self.decoder, data, &opts)?;
        Ok(&self.dst_ring[idx])
    }
}
