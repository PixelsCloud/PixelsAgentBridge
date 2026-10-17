//! Private ABI used only inside native media workers. Opaque codec
//! handles have one owner; returned buffers live until its next call or drop.
use crate::software::{SoftwareDecoder, SoftwareEncoder};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[repr(C)]
pub struct Bytes {
    data: *const u8,
    len: usize,
}

#[unsafe(no_mangle)]
extern "C" fn pab_soft_encoder_new(width: u32, height: u32, fps: u16) -> *mut SoftwareEncoder {
    catch_unwind(|| {
        SoftwareEncoder::new(width as usize, height as usize, fps)
            .ok()
            .map_or(std::ptr::null_mut(), |value| Box::into_raw(Box::new(value)))
    })
    .unwrap_or(std::ptr::null_mut())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn pab_soft_encoder_drop(value: *mut SoftwareEncoder) {
    if !value.is_null() {
        // SAFETY: native RAII passes its uniquely owned, live handle once.
        drop(unsafe { Box::from_raw(value) });
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn pab_soft_encode(
    value: *mut SoftwareEncoder,
    data: *const u8,
    len: usize,
    stride: usize,
    output: *mut Bytes,
) -> bool {
    if value.is_null() || data.is_null() || output.is_null() || len > 256 * 1024 * 1024 {
        return false;
    }
    // SAFETY: the worker keeps its mapped texture/PipeWire buffer and unique
    // codec handle alive throughout this call, with validated pitch and extent.
    let (value, input, output) = unsafe {
        (
            &mut *value,
            std::slice::from_raw_parts(data, len),
            &mut *output,
        )
    };
    output.data = std::ptr::null();
    output.len = 0;
    catch_unwind(AssertUnwindSafe(|| {
        match value.encode_bgra(input, stride) {
            Ok(packet) => {
                output.data = packet.as_ptr();
                output.len = packet.len();
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn pab_soft_encode_nv12(
    value: *mut SoftwareEncoder,
    y: *const u8,
    y_len: usize,
    y_stride: usize,
    uv: *const u8,
    uv_len: usize,
    uv_stride: usize,
    output: *mut Bytes,
) -> bool {
    if value.is_null()
        || y.is_null()
        || uv.is_null()
        || output.is_null()
        || y_len > 256 * 1024 * 1024
        || uv_len > 256 * 1024 * 1024
    {
        return false;
    }
    // SAFETY: the native worker holds a read-only CVPixelBuffer mapping and
    // its exclusive encoder handle until this call returns. Extents are capped.
    let (value, y, uv, output) = unsafe {
        (
            &mut *value,
            std::slice::from_raw_parts(y, y_len),
            std::slice::from_raw_parts(uv, uv_len),
            &mut *output,
        )
    };
    output.data = std::ptr::null();
    output.len = 0;
    catch_unwind(AssertUnwindSafe(|| {
        match value.encode_nv12(y, y_stride, uv, uv_stride) {
            Ok(packet) => {
                output.data = packet.as_ptr();
                output.len = packet.len();
                true
            }
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}
#[unsafe(no_mangle)]
extern "C" fn pab_soft_decoder_new(width: u32, height: u32) -> *mut SoftwareDecoder {
    catch_unwind(|| {
        SoftwareDecoder::new(width as usize, height as usize)
            .ok()
            .map_or(std::ptr::null_mut(), |value| Box::into_raw(Box::new(value)))
    })
    .unwrap_or(std::ptr::null_mut())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn pab_soft_decoder_drop(value: *mut SoftwareDecoder) {
    if !value.is_null() {
        // SAFETY: native RAII passes its uniquely owned, live handle once.
        drop(unsafe { Box::from_raw(value) });
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn pab_soft_decode(
    value: *mut SoftwareDecoder,
    data: *const u8,
    len: usize,
    output: *mut Bytes,
) -> bool {
    if value.is_null()
        || data.is_null()
        || output.is_null()
        || len == 0
        || len > pab_protocol::DESKTOP_MAX_FRAME_BYTES
    {
        return false;
    }
    // SAFETY: native packet storage and output struct outlive the call; the
    // codec handle is exclusively used on this worker thread.
    let (value, input, output) = unsafe {
        (
            &mut *value,
            std::slice::from_raw_parts(data, len),
            &mut *output,
        )
    };
    output.data = std::ptr::null();
    output.len = 0;
    catch_unwind(AssertUnwindSafe(|| match value.decode(input) {
        Ok(Some(frame)) => {
            output.data = frame.as_ptr();
            output.len = frame.len();
            true
        }
        Ok(None) => true,
        Err(_) => false,
    }))
    .unwrap_or(false)
}
