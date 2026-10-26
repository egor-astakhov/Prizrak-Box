//! Executable icons for the connections "Processes" view — port of
//! `src-wails/services/fileicon_{darwin,windows,linux}.go` (Electron's
//! app.getFileIcon). Results, including "no icon", are cached per path.

use std::collections::HashMap;
use std::sync::OnceLock;

use image::RgbaImage;
use parking_lot::Mutex;

const SIZE: u32 = 64;

fn cache() -> &'static Mutex<HashMap<String, Option<RgbaImage>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<RgbaImage>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn icon(path: &str) -> Option<RgbaImage> {
    if path.is_empty() {
        return None;
    }
    if let Some(v) = cache().lock().get(path) {
        return v.clone();
    }
    let r = extract(path).map(|i| {
        if i.width() != SIZE {
            image::imageops::resize(&i, SIZE, SIZE, image::imageops::FilterType::Triangle)
        } else {
            i
        }
    });
    cache().lock().insert(path.to_string(), r.clone());
    r
}

#[cfg(target_os = "macos")]
fn extract(path: &str) -> Option<RgbaImage> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;
    // An executable inside a bundle gets the bundle's icon.
    let p = match path.to_lowercase().find(".app/") {
        Some(i) => &path[..i + 4],
        None => path,
    };
    objc2::rc::autoreleasepool(|_| {
        let ws = NSWorkspace::sharedWorkspace();
        let img = ws.iconForFile(&NSString::from_str(p));
        let tiff = img.TIFFRepresentation()?;
        let bytes = tiff.to_vec();
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Tiff).ok().map(|i| i.to_rgba8())
    })
}

#[cfg(windows)]
fn extract(path: &str) -> Option<RgbaImage> {
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut hicon: HICON = std::ptr::null_mut();
        let mut id: u32 = 0;
        let n = PrivateExtractIconsW(wide.as_ptr(), 0, SIZE as i32, SIZE as i32, &mut hicon, &mut id, 1, 0);
        if n == 0 || hicon.is_null() {
            let mut large: HICON = std::ptr::null_mut();
            let c = ExtractIconExW(wide.as_ptr(), 0, &mut large, std::ptr::null_mut(), 1);
            if c == 0 || large.is_null() {
                return None;
            }
            hicon = large;
        }
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut ii) == 0 {
            DestroyIcon(hicon);
            return None;
        }
        let hdc = GetDC(std::ptr::null_mut());
        let mut bmp: BITMAP = std::mem::zeroed();
        GetObjectW(ii.hbmColor as _, std::mem::size_of::<BITMAP>() as i32, &mut bmp as *mut _ as *mut _);
        let (w, h) = (bmp.bmWidth.max(1) as u32, bmp.bmHeight.max(1) as u32);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w as i32;
        bi.bmiHeader.biHeight = -(h as i32);
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let ok = GetDIBits(hdc, ii.hbmColor, 0, h, buf.as_mut_ptr() as *mut _, &mut bi, DIB_RGB_COLORS);
        ReleaseDC(std::ptr::null_mut(), hdc);
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as _);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as _);
        }
        DestroyIcon(hicon);
        if ok == 0 {
            return None;
        }
        let has_alpha = buf.chunks(4).any(|p| p[3] != 0);
        for p in buf.chunks_mut(4) {
            p.swap(0, 2);
            if !has_alpha {
                p[3] = 255;
            }
        }
        RgbaImage::from_raw(w, h, buf)
    }
}

#[cfg(target_os = "linux")]
fn extract(path: &str) -> Option<RgbaImage> {
    let exe = std::path::Path::new(path).file_name()?.to_string_lossy().to_string();
    let icon = icon_name_for_exe(&exe).unwrap_or_else(|| exe.clone());
    let file = resolve_icon_file(&icon)?;
    let bytes = std::fs::read(file).ok()?;
    crate::images::decode(&bytes, SIZE)
}

#[cfg(target_os = "linux")]
fn data_dirs() -> Vec<std::path::PathBuf> {
    let mut v = vec![];
    match std::env::var("XDG_DATA_HOME") {
        Ok(h) if !h.is_empty() => v.push(h.into()),
        _ => {
            if let Some(h) = dirs::home_dir() {
                v.push(h.join(".local/share"));
            }
        }
    }
    let sys = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').map(std::path::PathBuf::from));
    v.push("/var/lib/flatpak/exports/share".into());
    v
}

#[cfg(target_os = "linux")]
fn icon_name_for_exe(exe: &str) -> Option<String> {
    let lower = exe.to_lowercase();
    for base in data_dirs() {
        let Ok(rd) = std::fs::read_dir(base.join("applications")) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "desktop").unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let (mut icon, mut exec) = (String::new(), String::new());
            let mut main = false;
            for line in text.lines() {
                let l = line.trim();
                if l.starts_with('[') {
                    main = l == "[Desktop Entry]";
                    continue;
                }
                if !main {
                    continue;
                }
                if let Some(v) = l.strip_prefix("Icon=") {
                    icon = v.trim().to_string();
                } else if let Some(v) = l.strip_prefix("TryExec=") {
                    if exec.is_empty() {
                        exec = v.trim().rsplit('/').next().unwrap_or("").to_string();
                    }
                } else if let Some(v) = l.strip_prefix("Exec=") {
                    if exec.is_empty() {
                        let first = v.trim().split_whitespace().next().unwrap_or("");
                        exec = first.rsplit('/').next().unwrap_or("").to_string();
                    }
                }
            }
            if !exec.is_empty() && exec.to_lowercase() == lower && !icon.is_empty() {
                return Some(icon);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn resolve_icon_file(name: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(name);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let sizes = ["64x64", "48x48", "128x128", "256x256", "96x96", "72x72", "32x32", "24x24", "scalable"];
    let themes = ["hicolor", "Adwaita", "gnome", "breeze", "Papirus"];
    for base in data_dirs() {
        for theme in themes {
            for sz in sizes {
                for ext in ["png", "svg"] {
                    let c = base.join("icons").join(theme).join(sz).join("apps").join(format!("{name}.{ext}"));
                    if c.is_file() {
                        return Some(c);
                    }
                }
            }
        }
        for ext in ["png", "svg", "xpm"] {
            let c = base.join("pixmaps").join(format!("{name}.{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Reveal a file in the OS file manager (connections "Show in folder").
pub fn show_in_folder(path: &str) {
    if path.is_empty() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").args(["-R", path]).spawn();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let p = std::path::Path::new(path);
        let dir = p.parent().filter(|d| d.is_dir()).unwrap_or(p);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
