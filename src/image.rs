// Copyright 2025 Au-Zone Technologies Inc.
// SPDX-License-Identifier: Apache-2.0

//! Hardware-accelerated image management using NXP G2D.
//!
//! Provides DMA-heap buffer allocation, G2D surface creation, and
//! color space conversion (NV12/YUYV → RGBA) for the replay pipeline.

use core::fmt;
use dma_heap::{Heap, HeapKind};
use g2d_sys::{
    g2d_format, g2d_format_G2D_NV12, g2d_format_G2D_RGB888, g2d_format_G2D_RGBA8888,
    g2d_format_G2D_RGBX8888, g2d_format_G2D_YUYV, g2d_rotation_G2D_ROTATION_0, G2DPhysical,
    G2DSurface, G2D,
};
use log::warn;
use std::{
    error::Error,
    ffi::c_void,
    io,
    os::fd::AsRawFd,
    ptr::null_mut,
    slice::from_raw_parts_mut,
};
use videostream::{encoder::VSLRect, fourcc::FourCC, frame::Frame};

pub const RGB3: FourCC = FourCC(*b"RGB3");
pub const RGBX: FourCC = FourCC(*b"RGBX");
pub const RGBA: FourCC = FourCC(*b"RGBA");
pub const YUYV: FourCC = FourCC(*b"YUYV");
pub const NV12: FourCC = FourCC(*b"NV12");

pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl From<VSLRect> for Rect {
    fn from(value: VSLRect) -> Self {
        Rect {
            x: value.x(),
            y: value.y(),
            width: value.width(),
            height: value.height(),
        }
    }
}

/// Convert a videostream FourCC to a G2D format constant.
fn fourcc_to_g2d_format(fourcc: FourCC) -> Result<g2d_format, io::Error> {
    match fourcc {
        RGB3 => Ok(g2d_format_G2D_RGB888),
        RGBX => Ok(g2d_format_G2D_RGBX8888),
        RGBA => Ok(g2d_format_G2D_RGBA8888),
        YUYV => Ok(g2d_format_G2D_YUYV),
        NV12 => Ok(g2d_format_G2D_NV12),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported FourCC: {fourcc}"),
        )),
    }
}

/// Bytes per pixel row for the given format and width.
const fn format_row_stride(format: FourCC, width: u32) -> usize {
    match format {
        RGB3 => 3 * width as usize,
        RGBX | RGBA => 4 * width as usize,
        YUYV => 2 * width as usize,
        // NV12: Y plane stride is width, but total size is width * height * 3/2
        NV12 => width as usize,
        _ => 4 * width as usize, // default to 4 bpp
    }
}

/// Total buffer size in bytes for the given dimensions and format.
pub const fn image_size(width: u32, height: u32, format: FourCC) -> usize {
    match format {
        NV12 => (width as usize) * (height as usize) * 3 / 2,
        _ => format_row_stride(format, width) * height as usize,
    }
}

/// Build a `G2DSurface` from an `Image` for use with G2D blit operations.
pub fn image_to_surface(img: &Image) -> Result<G2DSurface, Box<dyn Error>> {
    let phys = G2DPhysical::new(img.raw_fd())?;
    let addr = phys.address();
    let format = fourcc_to_g2d_format(img.format)?;

    let planes = match img.format {
        NV12 => {
            let y_size = img.width as u64 * img.height as u64;
            [addr, addr + y_size, 0]
        }
        _ => [addr, 0, 0],
    };

    Ok(G2DSurface {
        format,
        planes,
        left: 0,
        top: 0,
        right: img.width as i32,
        bottom: img.height as i32,
        stride: img.width as i32,
        width: img.width as i32,
        height: img.height as i32,
        blendfunc: 0,
        global_alpha: 255,
        clrcolor: 0,
        rot: g2d_rotation_G2D_ROTATION_0,
    })
}

/// Build a `G2DSurface` from a videostream `Frame` (VPU decoder output).
pub fn frame_to_surface(frame: &Frame) -> Result<G2DSurface, Box<dyn Error>> {
    let phys: G2DPhysical = match frame.paddr().ok().flatten() {
        Some(v) => (v as u64).into(),
        None => G2DPhysical::new(frame.handle()?)?,
    };
    let addr = phys.address();
    let fourcc_val = frame.fourcc().unwrap_or(0);
    let fourcc = FourCC::from(fourcc_val);
    let format = fourcc_to_g2d_format(fourcc)?;
    let width = frame.width().unwrap_or(0);
    let height = frame.height().unwrap_or(0);

    let planes = match fourcc {
        NV12 => {
            let y_size = width as u64 * height as u64;
            [addr, addr + y_size, 0]
        }
        _ => [addr, 0, 0],
    };

    Ok(G2DSurface {
        format,
        planes,
        left: 0,
        top: 0,
        right: width,
        bottom: height,
        stride: width,
        width,
        height,
        blendfunc: 0,
        global_alpha: 255,
        clrcolor: 0,
        rot: g2d_rotation_G2D_ROTATION_0,
    })
}

/// Perform a G2D blit from a VPU Frame to an Image, with optional crop.
pub fn convert_frame(
    g2d: &G2D,
    frame: &Frame,
    dest: &Image,
    crop: Option<&Rect>,
) -> Result<(), Box<dyn Error>> {
    let mut src = frame_to_surface(frame)?;
    if let Some(r) = crop {
        src.left = r.x;
        src.top = r.y;
        src.right = r.x + r.width;
        src.bottom = r.y + r.height;
    }
    let dst = image_to_surface(dest)?;
    g2d.blit(&src, &dst)?;
    g2d.finish()?;
    // NOTE: g2d_finish() synchronizes the G2D hardware but does NOT invalidate
    // CPU caches. On cached CMA heaps, the consumer must perform
    // DMA_BUF_IOCTL_SYNC with DRM PRIME attachment for correct reads.
    // See g2d-rs ARCHITECTURE.md for the full cache coherency protocol.
    Ok(())
}

#[derive(Debug)]
pub struct Image {
    fd: std::os::unix::io::OwnedFd,
    width: u32,
    height: u32,
    format: FourCC,
}

impl Image {
    pub fn new(width: u32, height: u32, format: FourCC) -> Result<Self, Box<dyn Error>> {
        let heap = Heap::new(HeapKind::Cma)?;
        let fd = heap.allocate(image_size(width, height, format))?;
        Ok(Self {
            fd,
            width,
            height,
            format,
        })
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> FourCC {
        self.format
    }

    pub fn size(&self) -> usize {
        image_size(self.width, self.height, self.format)
    }

    /// Create a persistent memory mapping of this image buffer.
    pub fn mmap(&self) -> Result<MappedImage, io::Error> {
        let size = self.size();
        let ptr = unsafe {
            libc::mmap(
                null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                self.raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(MappedImage {
            mmap: ptr as *mut u8,
            len: size,
        })
    }
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}x{} {} fd:{:?}",
            self.width, self.height, self.format, self.fd
        )
    }
}

pub struct MappedImage {
    mmap: *mut u8,
    len: usize,
}

impl MappedImage {
    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { from_raw_parts_mut(self.mmap, self.len) }
    }
}

impl Drop for MappedImage {
    fn drop(&mut self) {
        if unsafe { libc::munmap(self.mmap.cast::<c_void>(), self.len) } != 0 {
            warn!("munmap failed: {:?}", io::Error::last_os_error());
        }
    }
}
