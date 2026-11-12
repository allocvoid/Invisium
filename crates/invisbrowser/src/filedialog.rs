//! Native "open file" dialog used by `<input type=file>`.
//!
//! Windows: `GetOpenFileNameW` from comdlg32, with the FFI declared by hand (no extra crates).
//! Other platforms: no dialog yet, so the picker returns an empty list.

use std::path::PathBuf;

/// Show the platform's "open file" dialog and return the files the user chose (empty if the
/// dialog was cancelled or is not available on this platform). Blocks until it is closed.
/// `owner` is the native handle of the window the dialog belongs to (an HWND on Windows), or 0.
/// `accept` is the HTML `accept` attribute of the file control ("" = any file), e.g.
/// ".png,.jpg", "image/*", "image/png, application/pdf, .txt".
pub fn pick_files(owner: isize, multiple: bool, accept: &str) -> Vec<PathBuf> {
    imp::pick_files(owner, multiple, accept)
}

/// Size of the result buffer in UTF-16 units.
#[cfg_attr(not(windows), allow(dead_code))]
const FILE_BUF_LEN: usize = 32_768;

/// Turn an HTML `accept` attribute into a lower-case, de-duplicated list of extensions
/// (without the leading dot). Unknown MIME types and odd extensions are dropped.
#[cfg_attr(not(windows), allow(dead_code))]
fn accept_extensions(accept: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |ext: &str, out: &mut Vec<String>| {
        if !ext.is_empty()
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
            && !out.iter().any(|e| e == ext)
        {
            out.push(ext.to_string());
        }
    };
    for raw in accept.split(',') {
        let entry = raw.trim().to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if let Some(ext) = entry.strip_prefix('.') {
            push(ext, &mut out);
            continue;
        }
        let mapped: &[&str] = match entry.as_str() {
            "image/*" => &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif"],
            "video/*" => &["mp4", "webm", "mkv", "mov", "avi"],
            "audio/*" => &["mp3", "wav", "ogg", "flac", "m4a", "aac"],
            "text/*" => &["txt", "csv", "html", "htm", "css", "js", "json", "xml", "md"],
            "application/pdf" => &["pdf"],
            "application/zip" => &["zip"],
            "application/json" => &["json"],
            "image/png" => &["png"],
            "image/jpeg" => &["jpg", "jpeg"],
            "image/gif" => &["gif"],
            "image/webp" => &["webp"],
            "image/svg+xml" => &["svg"],
            "text/plain" => &["txt"],
            "text/csv" => &["csv"],
            "text/html" => &["html", "htm"],
            _ => &[],
        };
        for ext in mapped {
            push(ext, &mut out);
        }
    }
    out
}

/// Build the double-NUL-terminated filter string for `OPENFILENAMEW::lpstrFilter`.
#[cfg_attr(not(windows), allow(dead_code))]
fn build_filter(accept: &str) -> String {
    let exts = accept_extensions(accept);
    if exts.is_empty() {
        return "All files\0*.*\0\0".to_string();
    }
    let pattern = exts
        .iter()
        .map(|e| format!("*.{e}"))
        .collect::<Vec<_>>()
        .join(";");
    format!("Accepted files\0{pattern}\0All files\0*.*\0\0")
}

/// Parse the buffer `GetOpenFileNameW` filled in. Single selection: one full path. Multi
/// selection of several files: directory, then file names, each NUL-terminated, then an extra
/// NUL. Never reads past `buf`; a missing terminator just ends the last segment.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_result(buf: &[u16], multiple: bool) -> Vec<PathBuf> {
    let mut segments: Vec<String> = Vec::new();
    for seg in buf.split(|&c| c == 0) {
        if seg.is_empty() {
            break;
        }
        segments.push(String::from_utf16_lossy(seg));
        if !multiple {
            break;
        }
    }
    match segments.len() {
        0 => Vec::new(),
        1 => vec![PathBuf::from(&segments[0])],
        _ => {
            let dir = PathBuf::from(&segments[0]);
            segments[1..].iter().map(|name| dir.join(name)).collect()
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{build_filter, parse_result, FILE_BUF_LEN};
    use std::path::PathBuf;

    const OFN_HIDEREADONLY: u32 = 0x0000_0004;
    const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
    const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
    const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_EXPLORER: u32 = 0x0008_0000;
    const FNERR_BUFFERTOOSMALL: u32 = 0x3003;

    /// Mirror of the Windows SDK `OPENFILENAMEW` (commdlg.h), field for field.
    #[repr(C)]
    #[allow(non_snake_case)]
    struct OPENFILENAMEW {
        lStructSize: u32,
        hwndOwner: isize,
        hInstance: isize,
        lpstrFilter: *const u16,
        lpstrCustomFilter: *mut u16,
        nMaxCustFilter: u32,
        nFilterIndex: u32,
        lpstrFile: *mut u16,
        nMaxFile: u32,
        lpstrFileTitle: *mut u16,
        nMaxFileTitle: u32,
        lpstrInitialDir: *const u16,
        lpstrTitle: *const u16,
        Flags: u32,
        nFileOffset: u16,
        nFileExtension: u16,
        lpstrDefExt: *const u16,
        lCustData: isize,
        lpfnHook: isize,
        lpTemplateName: *const u16,
        pvReserved: *mut core::ffi::c_void,
        dwReserved: u32,
        FlagsEx: u32,
    }

    #[link(name = "comdlg32")]
    extern "system" {
        fn GetOpenFileNameW(ofn: *mut OPENFILENAMEW) -> i32;
        fn CommDlgExtendedError() -> u32;
    }

    pub(super) fn pick_files(owner: isize, multiple: bool, accept: &str) -> Vec<PathBuf> {
        let filter: Vec<u16> = build_filter(accept).encode_utf16().collect();
        let mut file_buf: Vec<u16> = vec![0; FILE_BUF_LEN];

        let mut flags = OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_HIDEREADONLY;
        if multiple {
            flags |= OFN_ALLOWMULTISELECT;
        }

        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            hInstance: 0,
            lpstrFilter: filter.as_ptr(),
            lpstrCustomFilter: std::ptr::null_mut(),
            nMaxCustFilter: 0,
            nFilterIndex: 1,
            lpstrFile: file_buf.as_mut_ptr(),
            nMaxFile: FILE_BUF_LEN as u32,
            lpstrFileTitle: std::ptr::null_mut(),
            nMaxFileTitle: 0,
            lpstrInitialDir: std::ptr::null(),
            lpstrTitle: std::ptr::null(),
            Flags: flags,
            nFileOffset: 0,
            nFileExtension: 0,
            lpstrDefExt: std::ptr::null(),
            lCustData: 0,
            lpfnHook: 0,
            lpTemplateName: std::ptr::null(),
            pvReserved: std::ptr::null_mut(),
            dwReserved: 0,
            FlagsEx: 0,
        };

        // SAFETY: `ofn` is a correctly laid out OPENFILENAMEW with lStructSize set; the filter
        // and file buffers it points to are owned locals that outlive this blocking call, and
        // nMaxFile matches the file buffer's length in UTF-16 units.
        let ok = unsafe { GetOpenFileNameW(&mut ofn) };
        if ok == 0 {
            // SAFETY: plain FFI call with no arguments; it only reads thread-local error state.
            let err = unsafe { CommDlgExtendedError() };
            if err == FNERR_BUFFERTOOSMALL {
                // Too many files for the buffer: the contents are truncated, so use none of it.
                return Vec::new();
            }
            // 0 = the user cancelled; any other code is a dialog failure. Either way, no files.
            return Vec::new();
        }
        parse_result(&file_buf, multiple)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    pub(super) fn pick_files(_owner: isize, _multiple: bool, _accept: &str) -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn filter_empty_accept() {
        assert_eq!(build_filter(""), "All files\0*.*\0\0");
        assert_eq!(build_filter("  ,  "), "All files\0*.*\0\0");
    }

    #[test]
    fn filter_extensions_lowercased() {
        assert_eq!(
            build_filter(".png, .JPG"),
            "Accepted files\0*.png;*.jpg\0All files\0*.*\0\0"
        );
    }

    #[test]
    fn filter_image_wildcard() {
        assert_eq!(
            build_filter("image/*"),
            "Accepted files\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.bmp;*.svg;*.ico;*.avif\0All files\0*.*\0\0"
        );
    }

    #[test]
    fn filter_mix_with_unknown_mime() {
        assert_eq!(
            build_filter("image/png, application/x-unknown, application/pdf, .txt"),
            "Accepted files\0*.png;*.pdf;*.txt\0All files\0*.*\0\0"
        );
        assert_eq!(build_filter("application/x-unknown"), "All files\0*.*\0\0");
    }

    #[test]
    fn filter_drops_odd_characters() {
        assert_eq!(build_filter(".p*g, .a;b"), "All files\0*.*\0\0");
        assert_eq!(
            build_filter(".p*g, .gif, .a;b, ."),
            "Accepted files\0*.gif\0All files\0*.*\0\0"
        );
    }

    #[test]
    fn parse_empty_buffer() {
        assert!(parse_result(&[], false).is_empty());
        assert!(parse_result(&[], true).is_empty());
        assert!(parse_result(&[0, 0, 0], true).is_empty());
    }

    #[test]
    fn parse_single_path() {
        let mut buf = w("C:\\dir\\a.png\0");
        buf.resize(64, 0);
        assert_eq!(parse_result(&buf, false), vec![PathBuf::from("C:\\dir\\a.png")]);
    }

    #[test]
    fn parse_multi_three_files() {
        let mut buf = w("C:\\dir\0a.png\0b.jpg\0c.txt\0\0");
        buf.resize(64, 0);
        assert_eq!(
            parse_result(&buf, true),
            vec![
                PathBuf::from("C:\\dir").join("a.png"),
                PathBuf::from("C:\\dir").join("b.jpg"),
                PathBuf::from("C:\\dir").join("c.txt"),
            ]
        );
    }

    #[test]
    fn parse_multi_one_file() {
        let mut buf = w("C:\\dir\\only.pdf\0\0");
        buf.resize(64, 0);
        assert_eq!(parse_result(&buf, true), vec![PathBuf::from("C:\\dir\\only.pdf")]);
    }

    #[test]
    fn parse_unterminated_buffer() {
        let buf = w("C:\\dir\\abc");
        assert_eq!(parse_result(&buf, false), vec![PathBuf::from("C:\\dir\\abc")]);
        let buf = w("C:\\dir\0a.png\0b");
        assert_eq!(parse_result(&buf, true).len(), 2);
    }
}
