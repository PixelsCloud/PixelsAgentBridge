//! Native access under one clipboard lock, with pre-allocation bounds and a
//! sequence check inside the write lock. No hidden file or shell handoff.
use super::clipboard_image::{decode, error, from_dib, optimize_png};
use pab_protocol::*;
use std::io;
use windows_sys::Win32::{
    Foundation::GlobalFree,
    System::{DataExchange::*, Memory::*, Ole::*},
};
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

// A valid owner is required by EmptyClipboard/SetClipboardData. Keep its
// message loop responsive even while the worker is blocked reading its pipe.
pub(super) struct Board(HWND, Option<std::thread::JoinHandle<()>>);
impl Board {
    pub(super) fn new() -> io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let class: Vec<_> = "STATIC\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if sender.send(hwnd as usize).is_err() || hwnd.is_null() {
                if !hwnd.is_null() {
                    DestroyWindow(hwnd);
                }
                return;
            }
            let mut message = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                if message.hwnd == hwnd && message.message == WM_CLOSE {
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            DestroyWindow(hwnd);
        });
        let hwnd = receiver
            .recv()
            .map_err(|_| error("clipboard_unavailable"))? as HWND;
        if hwnd.is_null() {
            let _ = thread.join();
            Err(error("clipboard_unavailable"))
        } else {
            Ok(Self(hwnd, Some(thread)))
        }
    }
    pub(super) fn write(
        &mut self,
        text: Option<&str>,
        image: Option<&DesktopClipboardImage>,
        expected: Option<u64>,
    ) -> io::Result<()> {
        write(self.0, text, image, expected)
    }
}
impl Drop for Board {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.0, WM_CLOSE, 0, 0);
        }
        if let Some(thread) = self.1.take() {
            let _ = thread.join();
        }
    }
}

struct Open;
impl Open {
    fn new(owner: HWND) -> io::Result<Self> {
        for _ in 0..3 {
            if unsafe { OpenClipboard(owner) } != 0 {
                return Ok(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        Err(error("clipboard_busy"))
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}
pub(super) fn sequence() -> Option<u64> {
    Some(unsafe { GetClipboardSequenceNumber() } as u64)
}
fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW([80u16, 78, 71, 0].as_ptr()) }
}
fn read_raw(format: u32, limit: usize) -> io::Result<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(format);
        if handle.is_null() {
            return Err(error("clipboard_unavailable"));
        }
        let size = GlobalSize(handle);
        if size == 0 || size > limit {
            return Err(error("clipboard_image_too_large"));
        }
        let pointer = GlobalLock(handle);
        if pointer.is_null() {
            return Err(error("clipboard_unavailable"));
        }
        let bytes = std::slice::from_raw_parts(pointer as *const u8, size).to_vec();
        GlobalUnlock(handle);
        Ok(bytes)
    }
}
pub(super) fn read() -> io::Result<(Option<String>, Option<DesktopClipboardImage>)> {
    let _open = Open::new(std::ptr::null_mut())?;
    let png = png_format();
    if png != 0 && unsafe { IsClipboardFormatAvailable(png) } != 0 {
        let data = read_raw(png, DESKTOP_CLIPBOARD_IMAGE_BYTES)?;
        if data.len() < 33 {
            return Err(error("clipboard_image_invalid"));
        }
        let image = DesktopClipboardImage {
            width: u32::from_be_bytes(data[16..20].try_into().unwrap()),
            height: u32::from_be_bytes(data[20..24].try_into().unwrap()),
            bytes: data.len() as u32,
            data,
        };
        // Validate decoded allocation/CRC before caching or forwarding native data.
        drop(_open);
        return optimize_png(image).map(|image| (None, Some(image)));
    }
    for format in [CF_DIBV5 as u32, CF_DIB as u32] {
        if unsafe { IsClipboardFormatAvailable(format) } != 0 {
            let data = read_raw(format, DESKTOP_CLIPBOARD_RGBA_BYTES + 4096)?;
            drop(_open);
            return from_dib(data).map(|image| (None, Some(image)));
        }
    }
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT as u32) } != 0 {
        let bytes = read_raw(CF_UNICODETEXT as u32, DESKTOP_CLIPBOARD_MAX_BYTES * 2 + 2)?;
        if bytes.len() % 2 != 0 {
            return Err(error("clipboard_text_invalid"));
        }
        let wide: Vec<_> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|c| *c != 0)
            .collect();
        let text = String::from_utf16(&wide).map_err(|_| error("clipboard_text_invalid"))?;
        validate_clipboard_text(&text).map_err(error)?;
        return Ok((Some(text), None));
    }
    Ok((None, None))
}
struct Memory(*mut std::ffi::c_void);
impl Memory {
    fn new(bytes: &[u8]) -> io::Result<Self> {
        unsafe {
            let memory = Self(GlobalAlloc(GMEM_MOVEABLE, bytes.len()));
            if memory.0.is_null() {
                return Err(error("clipboard_unavailable"));
            }
            let pointer = GlobalLock(memory.0);
            if pointer.is_null() {
                return Err(error("clipboard_unavailable"));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len());
            GlobalUnlock(memory.0);
            Ok(memory)
        }
    }
    fn publish(mut self, format: u32) -> io::Result<()> {
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            return Err(error("clipboard_write_failed"));
        }
        self.0 = std::ptr::null_mut();
        Ok(())
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}
fn write(
    owner: HWND,
    text: Option<&str>,
    image: Option<&DesktopClipboardImage>,
    expected_sequence: Option<u64>,
) -> io::Result<()> {
    let mut formats = Vec::new();
    if let Some(image) = image {
        let rgba = decode(image)?;
        let mut dib = vec![0u8; 124 + rgba.as_raw().len()];
        for (offset, value) in [
            (0, 124),
            (4, image.width),
            (8, (-(image.height as i32)) as u32),
            (16, 3),
            (20, rgba.as_raw().len() as u32),
            (40, 0x00ff0000),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
        ] {
            dib[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        for (source, dest) in rgba
            .as_raw()
            .chunks_exact(4)
            .zip(dib[124..].chunks_exact_mut(4))
        {
            dest.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
        }
        let png = png_format();
        if png == 0 {
            return Err(error("clipboard_unavailable"));
        }
        formats.push((png, Memory::new(&image.data)?));
        formats.push((CF_DIBV5 as u32, Memory::new(&dib)?));
    } else if let Some(text) = text {
        validate_clipboard_text(text).map_err(error)?;
        let data: Vec<_> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        formats.push((CF_UNICODETEXT as u32, Memory::new(&data)?));
    } else {
        return Err(error("clipboard_content_unavailable"));
    }
    let _open = Open::new(owner)?;
    if expected_sequence.is_some() && expected_sequence != sequence() {
        return Err(error("clipboard_changed"));
    }
    if unsafe { EmptyClipboard() } == 0 {
        return Err(error("clipboard_write_failed"));
    }
    for (format, memory) in formats {
        memory.publish(format)?;
    }
    Ok(())
}
