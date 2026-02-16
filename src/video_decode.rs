// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! H.264 and JPEG video decoding with hardware-accelerated color conversion.

use crate::image::{convert_frame, Image, MappedImage, RGBA};
use g2d_sys::G2D;
use log::{error, info, trace};
use std::{error::Error, io};
use turbojpeg::image::RgbaImage;
use videostream::decoder::{DecodeReturnCode, Decoder, DecoderCodec};

const BUF_COUNT: usize = 4;

pub struct VideoDecoder {
    decoder: Decoder,
    frames: Vec<Image>,
    mappings: Vec<MappedImage>,
    last_data: Vec<u8>,
    pub frame_count: usize,
}

impl VideoDecoder {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Ok(VideoDecoder {
            decoder: Decoder::create(DecoderCodec::H264, 30)?,
            frames: Vec::new(),
            mappings: Vec::new(),
            last_data: Vec::new(),
            frame_count: 0,
        })
    }

    fn allocate(&mut self) -> Result<(), Box<dyn Error>> {
        let crop = self.decoder.crop()?;
        info!("Video dimensions are: {}x{}", crop.width(), crop.height());
        for _ in 0..BUF_COUNT {
            trace!("Allocating frame");
            let image = Image::new(crop.width() as u32, crop.height() as u32, RGBA)?;
            self.frames.push(image);
            trace!("Done allocating frame");
        }
        Ok(())
    }

    fn allocate_jpeg(&mut self, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
        for _ in 0..BUF_COUNT {
            trace!("Allocating JPEG frame");
            let image = Image::new(width, height, RGBA)?;
            let mapping = image.mmap()?;
            self.mappings.push(mapping);
            self.frames.push(image);
            trace!("Done allocating JPEG frame");
        }
        Ok(())
    }

    pub fn decode_h264_msg(
        &mut self,
        data: &[u8],
        g2d: &G2D,
    ) -> Result<Option<&Image>, Box<dyn Error>> {
        let total_len = data.len();
        let mut consumed = 0;
        self.last_data.extend_from_slice(data);

        for _ in 0..3 {
            let (ret, bytes, frame) = match self.decoder.decode_frame(&self.last_data[consumed..]) {
                Ok(v) => v,
                Err(e) => {
                    error!("Could not decode frame: {:?}", e);
                    return Err(Box::new(e));
                }
            };
            trace!(
                "Consumed a total of {} out of {}",
                consumed + bytes,
                data.len()
            );
            if ret == DecodeReturnCode::Initialized {
                self.allocate()?;
            }
            if let Some(f) = frame {
                let index = self.frame_count % BUF_COUNT;
                if self.frames.is_empty() {
                    return Ok(None);
                }
                let crop_rect = self.decoder.crop()?.into();
                convert_frame(g2d, &f, &self.frames[index], Some(&crop_rect))?;
                self.frame_count += 1;
                self.last_data.clear();
                self.last_data.extend_from_slice(data);
                return Ok(Some(&self.frames[index]));
            }
            consumed += bytes;
        }

        self.last_data.clear();
        self.last_data.extend_from_slice(data);
        if consumed >= total_len {
            Ok(None)
        } else {
            Err(Box::new(io::Error::other("Could not decode video")))
        }
    }

    pub fn decode_jpeg_msg(
        &mut self,
        data: &[u8],
    ) -> Result<Option<&Image>, Box<dyn Error>> {
        let jpeg: RgbaImage = match turbojpeg::decompress_image(data) {
            Ok(v) => v,
            Err(e) => {
                error!("Could not decode frame: {:?}", e);
                return Err(Box::new(e));
            }
        };

        if self.frames.is_empty() {
            self.allocate_jpeg(jpeg.width(), jpeg.height())?;
        }

        let index = self.frame_count % BUF_COUNT;
        let frame_size = self.frames[index].size();
        let jpeg_size = jpeg.as_raw().len();

        if jpeg_size < frame_size {
            error!(
                "JPEG buffer size ({}) is smaller than frame size ({})",
                jpeg_size, frame_size
            );
            return Err(Box::new(io::Error::new(
                io::ErrorKind::InvalidData,
                "JPEG buffer too small for frame",
            )));
        }

        // Use the persistent mmap for this frame buffer
        let mapping = &mut self.mappings[index];
        mapping.as_slice_mut()[..frame_size]
            .copy_from_slice(&jpeg.as_raw()[..frame_size]);

        self.frame_count += 1;
        Ok(Some(&self.frames[index]))
    }
}
