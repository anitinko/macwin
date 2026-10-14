//! Values of the current user's `Run` key (programs started at login).

use std::ptr;

use macwin_platform::PlatformError;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn failed(what: &str, code: u32) -> PlatformError {
    PlatformError::Backend(format!("{what} failed (error {code})"))
}

/// The command registered under `name`, if any.
pub fn run_value(name: &str) -> Result<Option<String>, PlatformError> {
    let (key, value) = (wide(RUN_KEY), wide(name));
    let mut bytes = 0u32;
    // SAFETY: NUL-terminated names; a NULL buffer asks for the size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    match r {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        code => return Err(failed("RegGetValueW", code)),
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `len` bytes; the value is a NUL-terminated string.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    match r {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(None),
        code => return Err(failed("RegGetValueW", code)),
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Ok(Some(String::from_utf16_lossy(&buf[..end])))
}

/// Registers `command` under `name`.
pub fn set_run_value(name: &str, command: &str) -> Result<(), PlatformError> {
    let (key, value, data) = (wide(RUN_KEY), wide(name), wide(command));
    // SAFETY: NUL-terminated names; `data` is a NUL-terminated UTF-16
    // string of the given byte length.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(failed("RegSetKeyValueW", r));
    }
    Ok(())
}

/// Removes `name`; a missing value is fine.
pub fn delete_run_value(name: &str) -> Result<(), PlatformError> {
    let (key, value) = (wide(RUN_KEY), wide(name));
    // SAFETY: NUL-terminated names.
    let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
    match r {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        code => Err(failed("RegDeleteKeyValueW", code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses a value name of its own and removes it again.
    #[test]
    fn run_values_round_trip() {
        let name = format!("macwin-test-{}", std::process::id());
        assert_eq!(run_value(&name).unwrap(), None);
        set_run_value(&name, "\"C:\\Program Files\\x.exe\" --flag").unwrap();
        let got = run_value(&name);
        delete_run_value(&name).unwrap();
        assert_eq!(
            got.unwrap().as_deref(),
            Some("\"C:\\Program Files\\x.exe\" --flag")
        );
        assert_eq!(run_value(&name).unwrap(), None);
        delete_run_value(&name).unwrap();
    }
}
