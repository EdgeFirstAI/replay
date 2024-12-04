use crate::image::{G2DBuffer, Image, ImageManager, RGBA};
use log::{error, info, trace};
use nix::libc::{memcpy, mmap, munmap, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::{
    error::Error,
    io::{self, ErrorKind},
    os::raw::c_void,
};
use turbojpeg::image::RgbaImage;
use videostream::decoder::{DecodeReturnCode, Decoder, DecoderInputCodec};

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
            decoder: Decoder::create(DecoderInputCodec::H264, 30),
            frames: Vec::new(),
            g2dbufs: Vec::new(),
            last_data: Vec::new(),
            frame_count: 0,
        })
    }

    fn allocate(&mut self, imgmgr: &'a ImageManager) -> Result<(), Box<dyn Error>> {
        let crop = self.decoder.crop();
        info!(
            "Video dimensions are: {}x{}",
            crop.get_width(),
            crop.get_height()
        );
        for _ in 0..BUF_COUNT {
            trace!("Allocating frame");
            let dest_img_g2d_buf = match imgmgr.alloc(crop.get_width(), crop.get_height(), 4) {
                Ok(v) => v,
                Err(e) => {
                    error!("Could not allocate image on g2d: {:?}", e);
                    return Err(e);
                }
            };
            self.frames.push(Image::new_preallocated(
                imgmgr.g2d_buf_fd(&dest_img_g2d_buf),
                crop.get_width(),
                crop.get_height(),
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
                    return Err(e);
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
                match imgmgr.convert_phys(
                    &f,
                    &self.frames[index],
                    &Some(self.decoder.crop().into()),
                ) {
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
            Err(Box::new(io::Error::new(
                ErrorKind::Other,
                "Could not decode video",
            )))
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
                    jpeg.width() as i32,
                    jpeg.height() as i32,
                    RGBA,
                ));
                self.g2dbufs.push(dest_img_g2d_buf);

                trace!("Done reallocating frame");
            }
        };
        let index = self.frame_count % BUF_COUNT;

        unsafe {
            let mmap_ = mmap(
                std::ptr::null_mut(),
                self.frames[index].size(),
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                self.frames[index].raw_fd(),
                0,
            );
            memcpy(
                mmap_,
                jpeg.as_ptr() as *const c_void,
                self.frames[index].size(),
            );
            munmap(mmap_, self.frames[index].size());
        }
        Ok(Some(&self.frames[index]))
    }
}
