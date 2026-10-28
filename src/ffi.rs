//! C ABI surface — everything declared in include/pathkit.h.
//! Conventions (house standard):
//! - `*mut c_char` results are caller-owned; free with
//!   `pa_free_string`.
//! - Failures return null and set a thread-local last-error
//!   (code + message), readable via `pa_last_error` /
//!   `pa_last_error_code`.
//! - Nothing unwinds across the FFI boundary (catch_unwind
//!   everywhere; release profile also sets panic = "abort").
//! - Inputs must be non-null, valid UTF-8, NUL-terminated
//!   strings (the ABI contract in the header).

// The exported functions ARE the raw-pointer C ABI declared
// in include/pathkit.h; pointer validity is the documented
// contract.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

use crate::{PaResult, ERR_INTERNAL, ERR_INVALID_ARG, PA_OK};

thread_local! {
    static LAST_ERROR: RefCell<(i32, CString)> =
        RefCell::new((0, CString::new("").unwrap()));
}

pub fn set_error(code: i32, msg: &str) {
    let clean: String = msg.chars().filter(|&c| c != '\0').collect();
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = (
            code,
            CString::new(clean).unwrap_or_else(|_| CString::new("error").unwrap()),
        );
    });
}

fn clear_error() {
    set_error(PA_OK, "");
}

fn fail_null(code: i32, msg: &str) -> *mut c_char {
    set_error(code, msg);
    std::ptr::null_mut()
}

fn cstr<'a>(ptr: *const c_char, what: &str) -> PaResult<&'a str> {
    if ptr.is_null() {
        return Err((ERR_INVALID_ARG, format!("{what} must not be null")));
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| (ERR_INVALID_ARG, format!("{what} must be valid UTF-8")))
}

fn out_result(result: PaResult<String>) -> *mut c_char {
    match result {
        Ok(text) => match CString::new(text) {
            Ok(s) => {
                clear_error();
                s.into_raw()
            }
            Err(_) => fail_null(ERR_INTERNAL, "result string contained a NUL byte"),
        },
        Err((code, msg)) => fail_null(code, &msg),
    }
}

fn guard<F: FnOnce() -> *mut c_char>(f: F) -> *mut c_char {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(p) => p,
        Err(_) => fail_null(ERR_INTERNAL, "internal panic caught at the FFI boundary"),
    }
}

#[no_mangle]
pub extern "C" fn pa_version() -> *const c_char {
    static VERSION_C: &[u8] = b"0.1.0\0";
    VERSION_C.as_ptr() as *const c_char
}

#[no_mangle]
pub extern "C" fn pa_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().1.as_ptr())
}

#[no_mangle]
pub extern "C" fn pa_last_error_code() -> c_int {
    LAST_ERROR.with(|e| e.borrow().0) as c_int
}

/// # Safety (caller contract): `p` is null or a string this
/// library returned and has not been freed yet. Documented
/// in the header.
#[no_mangle]
pub extern "C" fn pa_free_string(p: *mut c_char) {
    if !p.is_null() {
        unsafe {
            drop(CString::from_raw(p));
        }
    }
}

#[no_mangle]
pub extern "C" fn pa_normalize(path: *const c_char, options_json: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let path = cstr(path, "path")?;
            let opts_text = if options_json.is_null() {
                None
            } else {
                Some(cstr(options_json, "options_json")?)
            };
            let opts = crate::parse_options(opts_text)?;
            Ok(crate::normalize(path, &opts).to_json())
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_validate(path: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let path = cstr(path, "path")?;
            Ok(crate::validate_json(path))
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_join(base: *const c_char, part: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let base = cstr(base, "base")?;
            let part = cstr(part, "part")?;
            Ok(crate::join(base, part)?.to_json())
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_relative(from_dir: *const c_char, to: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let from_dir = cstr(from_dir, "from_dir")?;
            let to = cstr(to, "to")?;
            crate::relative(from_dir, to)
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_to_extended(path: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let path = cstr(path, "path")?;
            crate::to_extended(path)
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_from_extended(path: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let path = cstr(path, "path")?;
            crate::from_extended(path)
        })();
        out_result(inner)
    })
}

#[no_mangle]
pub extern "C" fn pa_split(path: *const c_char) -> *mut c_char {
    guard(|| {
        let inner = (|| -> PaResult<String> {
            let path = cstr(path, "path")?;
            crate::split_json(path)
        })();
        out_result(inner)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn take(p: *mut c_char) -> String {
        assert!(
            !p.is_null(),
            "expected a result, got null (error: {:?})",
            unsafe { CStr::from_ptr(pa_last_error()) }
        );
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        pa_free_string(p);
        s
    }

    #[test]
    fn ffi_normalize_and_validate() {
        let out = pa_normalize(cs(r"C:\a\..\b").as_ptr(), std::ptr::null());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("normalized").unwrap().as_str(), Some("C:/b"));
        assert_eq!(v.get("kind").unwrap().as_str(), Some("drive_absolute"));

        let out = pa_normalize(cs(r"C:\a").as_ptr(), cs(r#"{"separator":"\\"}"#).as_ptr());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("normalized").unwrap().as_str(), Some(r"C:\a"));

        let out = pa_validate(cs(r"C:\CON.txt").as_ptr());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("valid").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn ffi_join_relative_split_extended() {
        let out = pa_join(cs(r"C:\a").as_ptr(), cs("b").as_ptr());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("windows_form").unwrap().as_str(), Some(r"C:\a\b"));

        let out = pa_relative(cs(r"C:\a\b").as_ptr(), cs(r"C:\a").as_ptr());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("relative").unwrap().as_str(), Some(".."));

        // Different roots: null + PA_ERR_DIFFERENT_ROOT.
        let out = pa_relative(cs(r"C:\a").as_ptr(), cs(r"D:\a").as_ptr());
        assert!(out.is_null());
        assert_eq!(pa_last_error_code(), crate::ERR_DIFFERENT_ROOT as c_int);

        let out = pa_split(cs(r"\\s\sh\a").as_ptr());
        let v = crate::json::parse(&take(out)).unwrap();
        assert_eq!(v.get("kind").unwrap().as_str(), Some("unc"));

        assert_eq!(take(pa_to_extended(cs(r"C:\a").as_ptr())), r"\\?\C:\a");
        assert_eq!(take(pa_from_extended(cs(r"\\?\C:\a").as_ptr())), r"C:\a");
    }

    #[test]
    fn ffi_null_and_parse_errors() {
        assert!(pa_normalize(std::ptr::null(), std::ptr::null()).is_null());
        assert_eq!(pa_last_error_code(), ERR_INVALID_ARG as c_int);
        assert!(pa_validate(std::ptr::null()).is_null());
        assert!(pa_join(std::ptr::null(), cs("x").as_ptr()).is_null());
        assert!(pa_to_extended(std::ptr::null()).is_null());
        assert!(pa_split(cs("").as_ptr()).is_null());
        assert_eq!(pa_last_error_code(), ERR_INVALID_ARG as c_int);
        // Bad options JSON is a parse error.
        assert!(pa_normalize(cs(r"C:\a").as_ptr(), cs("{").as_ptr()).is_null());
        assert_eq!(pa_last_error_code(), crate::ERR_PARSE as c_int);
        pa_free_string(std::ptr::null_mut()); // no-op, must not crash
    }
}
