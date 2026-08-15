//! The three NDK calls that let pixels be written straight into a Java
//! `Bitmap`.
//!
//! Declared here rather than taken from the `ndk` crate: it is three functions
//! with a stable ABI, and `#[link]` on the extern block is the whole of the
//! build configuration. What it buys is the thing `Backend.readRegion` promises
//! — the drawing thread gets a row copy out of the framebuffer into the bitmap
//! it is about to draw, with no intermediate buffer and no allocation.
//!
//! It is also what makes the destination offset free here and emulated there.
//! The RealVNC path cannot do either: `copyScaledRegion` writes into a
//! `ByteBuffer` their library allocates, so every region goes through a second
//! copy in `Bitmap.copyPixelsFromBuffer` — and that copy fills the whole
//! destination, so a read of part of a bitmap needs a scratch and a blit on
//! top. Here the offset
//! is `&mut bytes[dst_y * stride + dst_x * 4..]`.

use jni::sys::{JNIEnv, jobject};
use std::ffi::c_void;

pub const ANDROID_BITMAP_FORMAT_RGBA_8888: i32 = 1;

#[repr(C)]
#[derive(Default)]
pub struct AndroidBitmapInfo {
    pub width: u32,
    pub height: u32,
    /// Bytes per row, which is not always `width * 4`.
    pub stride: u32,
    pub format: i32,
    pub flags: u32,
}

#[link(name = "jnigraphics")]
unsafe extern "C" {
    fn AndroidBitmap_getInfo(env: *mut JNIEnv, bitmap: jobject, info: *mut AndroidBitmapInfo)
    -> i32;
    fn AndroidBitmap_lockPixels(env: *mut JNIEnv, bitmap: jobject, addr: *mut *mut c_void) -> i32;
    fn AndroidBitmap_unlockPixels(env: *mut JNIEnv, bitmap: jobject) -> i32;
}

/// A locked bitmap's pixels, unlocked when this is dropped.
///
/// The unlock has to happen on every path out, including the one where the
/// region turns out not to fit — a bitmap left locked is one Java can never
/// draw again.
pub struct Locked {
    env: *mut JNIEnv,
    bitmap: jobject,
    pub pixels: *mut u8,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Locked {
    /// Locks `bitmap`, or returns `None` if it is not an `ARGB_8888` one.
    ///
    /// # Safety
    ///
    /// `env` must be this thread's, and `bitmap` a live local or global
    /// reference to an `android.graphics.Bitmap`.
    pub unsafe fn new(env: *mut JNIEnv, bitmap: jobject) -> Option<Locked> {
        let mut info = AndroidBitmapInfo::default();
        if unsafe { AndroidBitmap_getInfo(env, bitmap, &mut info) } != 0 {
            return None;
        }
        if info.format != ANDROID_BITMAP_FORMAT_RGBA_8888 {
            return None;
        }
        let mut addr: *mut c_void = std::ptr::null_mut();
        if unsafe { AndroidBitmap_lockPixels(env, bitmap, &mut addr) } != 0 || addr.is_null() {
            return None;
        }
        Some(Locked {
            env,
            bitmap,
            pixels: addr as *mut u8,
            stride: info.stride as usize,
            width: info.width as usize,
            height: info.height as usize,
        })
    }

    /// The pixel bytes, as a slice covering every row this bitmap has.
    pub fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.pixels, self.stride * self.height) }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        unsafe { AndroidBitmap_unlockPixels(self.env, self.bitmap) };
    }
}
