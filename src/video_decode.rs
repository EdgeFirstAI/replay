// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::image::{G2DBuffer, Image, ImageManager, RGBA};
use log::{error, info, trace, warn};
use nix::libc::{memcpy, mmap, munmap, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::{error::Error, io, os::raw::c_void};
use turbojpeg::image::RgbaImage;
use videostream::decoder::{DecodeReturnCode, Decoder, DecoderCodec};

const BUF_COUNT: usize = 4;
pub struct VideoDecoder<'a> {
    decoder: Decoder,
    g2dbufs: Vec<G2DBuffer<'a>>,
    frames: Vec<Image>,
    // need to keep the data of at least the last frame for the decoder
    last_data: Vec<u8>,
    pub frame_count: usize,
}

impl<'a> VideoDecoder<'a> {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Ok(VideoDecoder {
            decoder: Decoder::create(DecoderCodec::H264, 30)?,
            frames: Vec::new(),
            g2dbufs: Vec::new(),
            last_data: Vec::new(),
            frame_count: 0,
        })
    }

    fn allocate(&mut self, imgmgr: &'a ImageManager) -> Result<(), Box<dyn Error>> {
        let crop = self.decoder.crop()?;
        info!("Video dimensions are: {}x{}", crop.width(), crop.height());
        for _ in 0..BUF_COUNT {
            trace!("Allocating frame");
            let dest_img_g2d_buf = match imgmgr.alloc(crop.width(), crop.height(), 4) {
                Ok(v) => v,
                Err(e) => {
                    error!("Could not allocate image on g2d: {:?}", e);
                    return Err(e);
                }
            };
            self.frames.push(Image::new_preallocated(
                imgmgr.g2d_buf_fd(&dest_img_g2d_buf),
                crop.width() as u32,
                crop.height() as u32,
                RGBA,
            ));
            self.g2dbufs.push(dest_img_g2d_buf);

            trace!("Done reallocating frame");
        }
        Ok(())
    }

    pub fn decode_h264_msg(
        &mut self,
        data: &[u8],
        imgmgr: &'a ImageManager,
    ) -> Result<Option<&Image>, Box<dyn Error>> {
        let total_len = data.len();
        let mut consumed = 0;
        self.last_data.extend_from_slice(data);
        // let mut image = None;
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
                self.allocate(imgmgr)?;
            }
            if let Some(f) = frame {
                let index = self.frame_count % BUF_COUNT;
                if self.frames.is_empty() {
                    return Ok(None);
                }
                let crop_rect = self.decoder.crop()?.into();
                match imgmgr.convert_phys(&f, &self.frames[index], &Some(crop_rect)) {
                    Ok(_) => {
                        trace!("Color space conversion success")
                    }
                    Err(e) => {
                        error!("Color space conversion failed: {:?}", e);
                        return Err(e);
                    }
                }
                self.frame_count += 1;
                // image = Some(&self.frames[index]);
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
        imgmgr: &'a ImageManager,
    ) -> Result<Option<&Image>, Box<dyn Error>> {
        // TODO: It looks like the VPU has a mjpeg encoder/decoder, investigate using
        // that?
        let jpeg: RgbaImage = match turbojpeg::decompress_image(data) {
            Ok(v) => v,
            Err(e) => {
                error!("Could not decode frame: {:?}", e);
                return Err(Box::new(e));
            }
        };
        if self.frames.is_empty() {
            for _ in 0..BUF_COUNT {
                trace!("Allocating frame");
                let dest_img_g2d_buf =
                    match imgmgr.alloc(jpeg.width() as i32, jpeg.height() as i32, 4) {
                        Ok(v) => v,
                        Err(e) => {
                            error!("Could not allocate image on g2d: {:?}", e);
                            return Err(e);
                        }
                    };
                self.frames.push(Image::new_preallocated(
                    imgmgr.g2d_buf_fd(&dest_img_g2d_buf),
                    jpeg.width(),
                    jpeg.height(),
                    RGBA,
                ));
                self.g2dbufs.push(dest_img_g2d_buf);

                trace!("Done reallocating frame");
            }
        };
        let index = self.frame_count % BUF_COUNT;
        let frame_size = self.frames[index].size();
        let jpeg_size = jpeg.as_raw().len();

        // Verify source buffer is large enough for the copy
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

        unsafe {
            let mmap_ = mmap(
                std::ptr::null_mut(),
                frame_size,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                self.frames[index].raw_fd(),
                0,
            );

            // Check for mmap failure
            if mmap_ == MAP_FAILED {
                let err = io::Error::last_os_error();
                error!("mmap failed: {:?}", err);
                return Err(Box::new(err));
            }

            memcpy(mmap_, jpeg.as_ptr() as *const c_void, frame_size);

            if munmap(mmap_, frame_size) != 0 {
                warn!("munmap failed: {:?}", io::Error::last_os_error());
            }
        }
        self.frame_count += 1;
        Ok(Some(&self.frames[index]))
    }
}
