//! Small, version-pinned Tesseract C API boundary for the disposable Android worker.
//! Models are passed as bytes from APK assets. No filesystem paths or network access.
use std::ffi::{CStr, c_char, c_int, c_void};

type Handle = *mut c_void;
type Init = unsafe extern "C" fn(
    Handle,
    *const u8,
    c_int,
    *const c_char,
    c_int,
    *mut *mut c_char,
    c_int,
    *mut *mut c_char,
    *mut *mut c_char,
    usize,
    c_int,
) -> c_int;

struct Library(Handle);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: owned successful dlopen, after the engine/text guards are gone.
        unsafe { libc::dlclose(self.0) };
    }
}
impl Library {
    fn symbol(&self, name: &CStr) -> Result<Handle, &'static str> {
        // SAFETY: handle and NUL-terminated symbol are valid for the pinned library.
        let address = unsafe { libc::dlsym(self.0, name.as_ptr()) };
        if address.is_null() {
            Err("ocr_runtime_unavailable")
        } else {
            Ok(address)
        }
    }
}
struct Engine {
    handle: Handle,
    delete: unsafe extern "C" fn(Handle),
}
impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: exactly one owned TessBaseAPI instance, library remains loaded.
        unsafe { (self.delete)(self.handle) };
    }
}
struct Text {
    pointer: *mut c_char,
    delete: unsafe extern "C" fn(*mut c_char),
}
impl Drop for Text {
    fn drop(&mut self) {
        // SAFETY: text allocated by TessBaseAPIGetUTF8Text and freed once.
        unsafe { (self.delete)(self.pointer) };
    }
}

pub fn recognize(
    gray: &[u8],
    width: i32,
    height: i32,
    model: &[u8],
) -> Result<(String, i32), &'static str> {
    if !(1..=2048).contains(&width)
        || !(1..=2048).contains(&height)
        || gray.len() != width as usize * height as usize
        || model.len() != 2_469_156
        || crate::filesystem::digest(model)
            != "a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730"
    {
        return Err("invalid_ocr_input_or_model");
    }
    // SAFETY: fixed APK library name, no caller-controlled search path. Java loads
    // its bundled dependencies first. All signatures match capi.h in Tesseract 5.
    unsafe {
        let library = Library(libc::dlopen(c"libtesseract.so".as_ptr(), libc::RTLD_NOW));
        if library.0.is_null() {
            // A null dlopen must not be passed to dlclose.
            std::mem::forget(library);
            return Err("ocr_runtime_unavailable");
        }
        let create: unsafe extern "C" fn() -> Handle =
            std::mem::transmute(library.symbol(c"TessBaseAPICreate")?);
        let delete = std::mem::transmute::<Handle, unsafe extern "C" fn(Handle)>(
            library.symbol(c"TessBaseAPIDelete")?,
        );
        let init: Init = std::mem::transmute(library.symbol(c"TessBaseAPIInit5")?);
        let set_image: unsafe extern "C" fn(Handle, *const u8, c_int, c_int, c_int, c_int) =
            std::mem::transmute(library.symbol(c"TessBaseAPISetImage")?);
        let resolution: unsafe extern "C" fn(Handle, c_int) =
            std::mem::transmute(library.symbol(c"TessBaseAPISetSourceResolution")?);
        let text: unsafe extern "C" fn(Handle) -> *mut c_char =
            std::mem::transmute(library.symbol(c"TessBaseAPIGetUTF8Text")?);
        let confidence: unsafe extern "C" fn(Handle) -> c_int =
            std::mem::transmute(library.symbol(c"TessBaseAPIMeanTextConf")?);
        let delete_text = std::mem::transmute::<Handle, unsafe extern "C" fn(*mut c_char)>(
            library.symbol(c"TessDeleteText")?,
        );
        let pointer = create();
        if pointer.is_null() {
            return Err("ocr_runtime_unavailable");
        }
        let engine = Engine {
            handle: pointer,
            delete,
        };
        let null = std::ptr::null_mut();
        if init(
            engine.handle,
            model.as_ptr(),
            model.len() as c_int,
            c"chi_sim".as_ptr(),
            1,
            null,
            0,
            null,
            null,
            0,
            0,
        ) != 0
        {
            return Err("ocr_model_unavailable");
        }
        set_image(engine.handle, gray.as_ptr(), width, height, 1, width);
        resolution(engine.handle, 200);
        let pointer = text(engine.handle);
        if pointer.is_null() {
            return Err("ocr_recognition_failed");
        }
        let text = Text {
            pointer,
            delete: delete_text,
        };
        let value = CStr::from_ptr(text.pointer)
            .to_str()
            .map_err(|_| "ocr_result_invalid")?;
        if value.len() > 1024 * 1024 {
            return Err("ocr_result_limit");
        }
        Ok((
            value.trim().to_owned(),
            confidence(engine.handle).clamp(0, 100),
        ))
    }
}
