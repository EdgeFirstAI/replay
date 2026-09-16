use crate::image::{G2DBuffer, Image, ImageManager, RGBA};
use log::{error, info, trace, warn};
use nix::libc::{memcpy, mmap, munmap, MAP_SHARED, PROT_READ, PROT_WRITE};
use std::{error::Error, io, os::raw::c_void};
use turbojpeg::image::RgbaImage;
use videostream::decoder::{CodecBackend, DecodeReturnCode, Decoder, DecoderCodec};

const BUF_COUNT: usize = 4;
pub struct VideoDecoder<'a> {
    decoder: Option<Decoder>,
    h264_failed: bool,
    g2dbufs: Vec<G2DBuffer<'a>>,
    frames: Vec<Image>,
    // need to keep the data of at least the last frame for the decoder
    last_data: Vec<u8>,
    pub frame_count: usize,
}

impl<'a> VideoDecoder<'a> {
    pub fn new() -> Self {
        VideoDecoder {
            decoder: None,
            h264_failed: false,
            frames: Vec::new(),
            g2dbufs: Vec::new(),
            last_data: Vec::new(),
            frame_count: 0,
        }
    }

    fn ensure_h264(&mut self) -> Result<Option<&Decoder>, Box<dyn Error>> {
        if self.h264_failed {
            return Ok(None);
        }
        if self.decoder.is_none() {
            match Decoder::create_ex(DecoderCodec::H264, 30, CodecBackend::V4L2) {
                Ok(decoder) => {
                    info!("Opened V4L2 H264 decoder");
                    self.decoder = Some(decoder);
                }
                Err(e) => {
                    warn!(
                        "V4L2 H264 decoder unavailable ({e}); \
                         vsiv4l2 /dev/video0,/dev/video1 stay EBUSY until vsidaemon \
                         holds /dev/vsi_daemon_ctrl. Skipping CameraFrame synthesis"
                    );
                    self.h264_failed = true;
                    return Ok(None);
                }
            }
        }
        Ok(self.decoder.as_ref())
    }

    fn allocate(&mut self, imgmgr: &'a ImageManager) -> Result<(), Box<dyn Error>> {
        let decoder = self
            .decoder
            .as_ref()
            .ok_or_else(|| io::Error::other("H264 decoder is not open"))?;
        let crop = decoder.crop()?;
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
        let Some(_) = self.ensure_h264()? else {
            return Ok(None);
        };
        self.last_data.extend_from_slice(data);

        // V4L2 stateful decoders (i.MX 8M Plus vsiv4l2 / Hantro) expect one
        // NAL / access unit per OUTPUT buffer. Feeding a whole MCAP payload
        // that spans several NALs produces no CAPTURE frames — the same
        // wedge camera replay already documented.
        loop {
            if let Some(nal_len) = next_nal_unit_len(&self.last_data) {
                if let Some(index) = self.feed_h264_nal(nal_len, imgmgr)? {
                    return Ok(Some(&self.frames[index]));
                }
                continue;
            }
            // Last NAL in this message has no follower start code. Treat the
            // remainder as a complete access unit, matching one Foxglove
            // CompressedVideo sample.
            if starts_with_start_code(&self.last_data) && !self.last_data.is_empty() {
                let remaining = self.last_data.len();
                if let Some(index) = self.feed_h264_nal(remaining, imgmgr)? {
                    return Ok(Some(&self.frames[index]));
                }
            }
            return Ok(None);
        }
    }

    fn feed_h264_nal(
        &mut self,
        nal_len: usize,
        imgmgr: &'a ImageManager,
    ) -> Result<Option<usize>, Box<dyn Error>> {
        let mut last_err = None;
        for _ in 0..8 {
            let decoded = {
                let decoder = self.decoder.as_ref().expect("H264 decoder opened");
                decoder.decode_frame(&self.last_data[..nal_len])
            };
            match decoded {
                Ok((code, used, frame)) => {
                    let used = if used == 0 {
                        nal_len.min(self.last_data.len())
                    } else {
                        used.min(nal_len).min(self.last_data.len())
                    };
                    // vsiv4l2 often sets FRAME_DEC without INIT_INFO. The Rust
                    // wrapper also reports only Initialized when both bits are
                    // set, so the first displayable frame must allocate too.
                    if self.frames.is_empty()
                        && (code == DecodeReturnCode::Initialized || frame.is_some())
                    {
                        self.allocate(imgmgr)?;
                    }
                    if let Some(f) = frame {
                        if self.frames.is_empty() {
                            self.last_data.drain(..used);
                            return Ok(None);
                        }
                        let index = self.frame_count % BUF_COUNT;
                        let crop = self.decoder.as_ref().expect("H264 decoder opened").crop()?;
                        if let Err(e) =
                            imgmgr.convert_phys(&f, &self.frames[index], &Some(crop.into()))
                        {
                            error!("Color space conversion failed: {:?}", e);
                            return Err(e);
                        }
                        self.frame_count += 1;
                        self.last_data.drain(..used);
                        return Ok(Some(index));
                    }
                    self.last_data.drain(..used);
                    return Ok(None);
                }
                Err(e) => {
                    // "no OUTPUT buffer available" is backpressure, not a
                    // corrupt bitstream. Retry before giving up this NAL.
                    last_err = Some(e);
                }
            }
        }
        if let Some(e) = last_err {
            error!("Could not decode frame: {:?}", e);
            self.last_data.drain(..nal_len.min(self.last_data.len()));
            return Err(e.into());
        }
        Ok(None)
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
        self.frame_count += 1;
        Ok(Some(&self.frames[index]))
    }
}

/// Length of the leading NAL in `buf`, ended by the next Annex-B start
/// code, or `None` if `buf` is not start-code aligned or the terminator
/// is not in view yet.
fn next_nal_unit_len(buf: &[u8]) -> Option<usize> {
    let leading_sc_len = leading_start_code_len(buf)?;
    let mut i = leading_sc_len + 1;
    while i + 2 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                return Some(i);
            }
            if buf[i + 2] == 0 && i + 3 < buf.len() && buf[i + 3] == 1 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn leading_start_code_len(buf: &[u8]) -> Option<usize> {
    if buf.len() >= 4 && buf[0] == 0 && buf[1] == 0 && buf[2] == 0 && buf[3] == 1 {
        Some(4)
    } else if buf.len() >= 3 && buf[0] == 0 && buf[1] == 0 && buf[2] == 1 {
        Some(3)
    } else {
        None
    }
}

fn starts_with_start_code(buf: &[u8]) -> bool {
    leading_start_code_len(buf).is_some()
}

#[cfg(test)]
mod tests {
    use super::{next_nal_unit_len, starts_with_start_code};

    #[test]
    fn next_nal_none_for_short_or_single_nal() {
        assert_eq!(next_nal_unit_len(&[0, 0, 0]), None);
        assert_eq!(next_nal_unit_len(&[0, 0, 0, 1, 0x67, 0xaa]), None);
        assert!(!starts_with_start_code(&[0xff, 0, 0, 1]));
    }

    #[test]
    fn next_nal_finds_4byte_then_3byte_boundary() {
        let mut buf = vec![0, 0, 0, 1, 0x67, 0x42, 0xe0];
        buf.extend_from_slice(&[0, 0, 0, 1, 0x28, 0xce]);
        assert_eq!(next_nal_unit_len(&buf), Some(7));

        let mut buf = vec![0, 0, 0, 1, 0x67, 0x42];
        buf.extend_from_slice(&[0, 0, 1, 0x28, 0xce]);
        assert_eq!(next_nal_unit_len(&buf), Some(6));
        assert!(starts_with_start_code(&buf));
    }
}
