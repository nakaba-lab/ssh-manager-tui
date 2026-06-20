//! Clipboard writes for secrets. On Windows we set the text together with the
//! documented exclusion formats so clipboard-history and cloud-sync agents skip
//! the value (#5); elsewhere we defer to arboard. The non-secret "copy ssh
//! command" path keeps using arboard directly in update.rs.

#[cfg(not(windows))]
pub fn set_secret(text: &str) -> std::io::Result<()> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(std::borrow::Cow::Borrowed(text)))
        .map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(windows)]
pub fn set_secret(text: &str) -> std::io::Result<()> {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    // GlobalFree lives in Win32::Foundation, NOT Win32::System::Memory.
    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    // Closes the clipboard on every exit path.
    struct Clip;
    impl Drop for Clip {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    // Allocate a GMEM_MOVEABLE block, copy `data` into it, and return the HGLOBAL.
    unsafe fn moveable(data: &[u8]) -> io::Result<HANDLE> {
        let h = unsafe { GlobalAlloc(GMEM_MOVEABLE, data.len()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let p = unsafe { GlobalLock(h) };
        if p.is_null() {
            unsafe { GlobalFree(h) };
            return Err(io::Error::last_os_error());
        }
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, data.len()) };
        unsafe { GlobalUnlock(h) };
        Ok(h as HANDLE)
    }

    // Zeroizing so this UTF-16 copy of the secret is scrubbed from our heap on
    // drop (the OS-owned HGLOBAL is unavoidable; this extra copy is not). (#5)
    let wide: zeroize::Zeroizing<Vec<u16>> = zeroize::Zeroizing::new(
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect(),
    );
    let text_bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };

    unsafe {
        if OpenClipboard(std::ptr::null_mut::<core::ffi::c_void>() as HWND) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _clip = Clip;
        if EmptyClipboard() == 0 {
            return Err(io::Error::last_os_error());
        }
        // The actual text. On success the system OWNS the HGLOBAL (do not free it);
        // on failure we must free it ourselves.
        let htext = moveable(text_bytes)?;
        if SetClipboardData(CF_UNICODETEXT as u32, htext).is_null() {
            let err = io::Error::last_os_error();
            GlobalFree(htext as _);
            return Err(err);
        }
        // Exclusion formats: any data on these registered formats tells history /
        // cloud-clipboard managers not to capture this clipboard. Best-effort.
        for name in [
            "ExcludeClipboardContentFromMonitorProcessing",
            "CanIncludeInClipboardHistory",
            "CanUploadToCloudClipboard",
        ] {
            let wname: Vec<u16> = OsStr::new(name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let fmt = RegisterClipboardFormatW(wname.as_ptr());
            if fmt != 0 {
                // A single zero DWORD payload signals "exclude".
                if let Ok(h) = moveable(&0u32.to_ne_bytes())
                    && SetClipboardData(fmt, h).is_null()
                {
                    GlobalFree(h as _);
                }
            }
        }
        Ok(())
    }
}
