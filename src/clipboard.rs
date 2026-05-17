// Clipboard abstraction — Windows and macOS implementations.
// Windows uses Win32 API; macOS uses NSPasteboard via objc2-app-kit.

// ─── Windows implementation ───

#[cfg(windows)]
pub fn read_fm_clipboard() -> Result<Vec<u8>, String> {
    use clipboard_win::raw::{get_clipboard_data, register_format, size};

    const FM_FORMATS: &[&str] = &["Mac-XMSS", "XMSS", "XMFN", "XMSC", "XMFD", "XMTB", "XMLO"];

    let _clip = clipboard_win::Clipboard::new_attempts(30)
        .map_err(|e| format!("Cannot open clipboard: {:?}", e))?;

    for &fmt_name in FM_FORMATS {
        let fmt = match register_format(fmt_name) {
            Some(f) => f,
            None => continue,
        };
        if let Ok(ptr) = get_clipboard_data(fmt.get()) {
            let fmt_size = size(fmt.get()).map(|s| s.get()).unwrap_or(0);
            if fmt_size == 0 { continue; }
            let data = unsafe { std::slice::from_raw_parts(ptr.as_ptr() as *const u8, fmt_size) };
            return Ok(data.to_vec());
        }
    }

    Err("No FM data in clipboard".to_string())
}

#[cfg(windows)]
pub fn write_fm_clipboard(data: &[u8]) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{HWND, FALSE, GetLastError},
        System::DataExchange::{OpenClipboard, CloseClipboard, EmptyClipboard, SetClipboardData, RegisterClipboardFormatA},
        System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
    };

    unsafe {
        let c_name = std::ffi::CString::new("XMSS").map_err(|e| e.to_string())?;
        let fmt = RegisterClipboardFormatA(c_name.as_ptr() as *const u8);
        if fmt == 0 {
            return Err(format!("RegisterClipboardFormatA(XMSS) failed, GetLastError={}", GetLastError()));
        }

        // Windows FM clipboard payload: 4-byte LE u32 length, then raw XML.
        let len = data.len() as u32;
        let mut framed = Vec::with_capacity(4 + data.len());
        framed.extend_from_slice(&len.to_le_bytes());
        framed.extend_from_slice(data);

        for _attempt in 0..30 {
            if OpenClipboard(0 as HWND) == FALSE {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }

            EmptyClipboard();

            let mem_size = framed.len();
            let hmem = GlobalAlloc(GMEM_MOVEABLE, mem_size);
            if hmem.is_null() {
                let code = GetLastError();
                CloseClipboard();
                return Err(format!("GlobalAlloc({} bytes) failed, GetLastError={}", mem_size, code));
            }

            let locked = GlobalLock(hmem) as *mut u8;
            if locked.is_null() {
                let code = GetLastError();
                CloseClipboard();
                return Err(format!("GlobalLock failed, GetLastError={}", code));
            }

            std::ptr::copy_nonoverlapping(framed.as_ptr(), locked, mem_size);
            GlobalUnlock(hmem);

            if SetClipboardData(fmt, hmem).is_null() {
                let code = GetLastError();
                CloseClipboard();
                return Err(format!("SetClipboardData(fmt={}) failed, GetLastError={}", fmt, code));
            }

            CloseClipboard();
            return Ok(());
        }

        Err("Cannot open clipboard after 30 attempts".to_string())
    }
}

#[cfg(windows)]
pub fn list_clipboard_formats() -> Vec<(u32, String, usize)> {
    use clipboard_win::raw::{format_name_big, size, EnumFormats};

    let _clip = match clipboard_win::Clipboard::new_attempts(30) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    EnumFormats::new()
        .filter_map(|fmt| {
            let name = format_name_big(fmt).unwrap_or_else(|| format!("Unknown({})", fmt));
            let fmt_size = size(fmt).map(|s| s.get()).unwrap_or(0);
            Some((fmt, name, fmt_size))
        })
        .collect()
}

// ─── macOS implementation using NSPasteboard via objc2 ───

#[cfg(target_os = "macos")]
pub fn read_fm_clipboard() -> Result<Vec<u8>, String> {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    let pb = NSPasteboard::generalPasteboard();
    let types = pb.types().ok_or("No types on pasteboard")?;

    // FM uses dynamic UTI strings on macOS
    let fm_utis = [
        "dyn.ah62d4rv4gk8zuxnxnq", // Script Step (XMSS)
        "dyn.ah62d4rv4gk8zuxnxkq", // Script (XMSC)
        "dyn.ah62d4rv4gk8zuxngku", // Field (XMFD)
        "dyn.ah62d4rv4gk8zuxnykk", // Table (XMTB)
        "dyn.ah62d4rv4gk8zuxngm2", // Custom Function (XMFN)
        "dyn.ah62d4rv4gk8zuxn0mu", // Value List (XMVL)
        "dyn.ah62d4rv4gk8zuxnqm6", // Layout Object fp7 (XMLO)
        "dyn.ah62d4rv4gk8zuxnqgk", // Layout Object fmp12 (XML2)
        "dyn.agk8u",                 // Theme (FM 17-2023)
        "dyn.ah62d4rv4gk8zuxnyma", // Theme (FM 2024)
    ];

    // Try FM dynamic UTIs first
    for uti in &fm_utis {
        let ns_type = NSString::from_str(uti);
        if let Some(data) = pb.dataForType(&ns_type) {
            let len = data.length();
            if len == 0 { continue; }
            let slice = nsdata_to_bytes(&data, len);
            return Ok(slice);
        }
    }

    // Fallback: scan all types for anything that looks like FM data
    for ns_type in types.iter() {
        let type_str = ns_type.to_string();
        if type_str.starts_with("dyn.ah62d4rv4gk8zuxn") || type_str == "dyn.agk8u" {
            if let Some(data) = pb.dataForType(&ns_type) {
                let len = data.length();
                if len == 0 { continue; }
                let slice = nsdata_to_bytes(&data, len);
                return Ok(slice);
            }
        }
    }

    Err("No FM data in clipboard".to_string())
}

#[cfg(target_os = "macos")]
fn nsdata_to_bytes(data: &objc2_foundation::NSData, len: usize) -> Vec<u8> {
    let bytes_ptr: *const std::ffi::c_void = unsafe {
        objc2::msg_send![data, bytes]
    };
    if bytes_ptr.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(bytes_ptr as *const u8, len) }.to_vec()
    }
}

#[cfg(target_os = "macos")]
pub fn write_fm_clipboard(data: &[u8]) -> Result<(), String> {
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSData, NSString, NSArray};

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    let xml_str = String::from_utf8_lossy(data);
    let uti = detect_fm_type(&xml_str);

    let ns_uti = NSString::from_str(uti);
    let text_type = NSString::from_str("public.utf8-plain-text");

    let types = NSArray::from_retained_slice(&[ns_uti.clone(), text_type.clone()]);

    // declareTypes_owner is unsafe because owner type must be correct, None is safe
    unsafe {
        pb.declareTypes_owner(&types, None);
    }

    let ns_data = unsafe {
        NSData::dataWithBytes_length(data.as_ptr() as *const std::ffi::c_void, data.len())
    };
    if !pb.setData_forType(Some(&ns_data), &ns_uti) {
        return Err("Failed to set clipboard data for FM type".to_string());
    }

    // Also write a plain text version for compatibility
    let text_start = xml_str.find('<').unwrap_or(0);
    let plain_text = &xml_str[text_start..];
    let ns_text = NSString::from_str(plain_text);
    pb.setString_forType(&ns_text, &text_type);

    Ok(())
}

#[cfg(target_os = "macos")]
pub fn list_clipboard_formats() -> Vec<(u32, String, usize)> {
    use objc2_app_kit::NSPasteboard;

    let pb = NSPasteboard::generalPasteboard();

    let types = match pb.types() {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut result = Vec::new();
    for (i, ns_type) in types.iter().enumerate() {
        let type_str = ns_type.to_string();
        let size = pb.dataForType(&ns_type).map(|d| d.length() as usize).unwrap_or(0);
        result.push((i as u32, type_str, size));
    }
    result
}

#[cfg(target_os = "macos")]
fn detect_fm_type(_xml: &str) -> &'static str {
    // Default to Script Step since it's the most common
    // TODO: parse XML to detect script vs script step vs field etc.
    "dyn.ah62d4rv4gk8zuxnxnq"
}

// ─── Fallback for other platforms ───

#[cfg(not(any(windows, target_os = "macos")))]
pub fn read_fm_clipboard() -> Result<Vec<u8>, String> {
    Err("Clipboard not supported on this platform".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn write_fm_clipboard(_data: &[u8]) -> Result<(), String> {
    Err("Clipboard not supported on this platform".to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn list_clipboard_formats() -> Vec<(u32, String, usize)> {
    Vec::new()
}