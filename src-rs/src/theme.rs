//! Theme resolution — port of `composables/useAppTheme.ts` and `util/theme.ts`.
//!
//! * the background (`menuStore.background`, a CSS `url(...)` value, or the
//!   active profile's pxd-theme image) is loaded from the bundled images, the
//!   uploaded custom images or the network;
//! * `analyze_image` picks the accent and the light/dark verdict exactly like
//!   the TS version (5-5-5 bins over a 64×64 downscale, …);
//! * a blurred copy is produced for the backdrop-blur emulation (glass.slint).

use image::{imageops, DynamicImage, RgbaImage};

use crate::assets;
use crate::paths;
use crate::store::{MenuState, ProfileTheme};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageTheme {
    pub h: f64,
    pub s: f64,
    pub l: f64,
    pub white: bool,
}

pub type Rgb = [f64; 3];

pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let k = |n: f64| (n + h / 30.0) % 12.0;
    let a = s * l.min(1.0 - l);
    let f = |n: f64| l - a * (-1f64).max((k(n) - 3.0).min(9.0 - k(n)).min(1.0));
    [f(0.0) * 255.0, f(8.0) * 255.0, f(4.0) * 255.0]
}

pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> Rgb {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let hh = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [(hh * 60.0 + 360.0) % 360.0, s, l]
}

pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let lum = |c: Rgb| {
        let v: Vec<f64> = c
            .iter()
            .map(|x| {
                let x = x / 255.0;
                if x <= 0.03928 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            })
            .collect();
        0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
    };
    let (l1, l2) = (lum(a), lum(b));
    (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
}

pub fn parse_color(c: &str) -> Option<Rgb> {
    let c = c.trim();
    if let Some(hex) = c.strip_prefix('#') {
        let full: String = if hex.len() == 3 {
            hex.chars().flat_map(|ch| [ch, ch]).collect()
        } else {
            hex.to_string()
        };
        if full.len() >= 6 {
            let p = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok().map(f64::from);
            return Some([p(0)?, p(2)?, p(4)?]);
        }
        return None;
    }
    let re = regex::Regex::new(r"rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)").ok()?;
    let m = re.captures(c)?;
    Some([m[1].parse().ok()?, m[2].parse().ok()?, m[3].parse().ok()?])
}

/// `ctx.drawImage(img, 0, 0, N, N)` as WKWebView (Wails on macOS) does it at
/// the default smoothing quality: the source pixel under the centre of each
/// target pixel, no filtering (matches WebKit's canvas byte for byte on
/// baseline JPEGs). The accent comes from a colour bin that may hold only
/// ~1 % of the samples, so an averaging filter picks another hue on several
/// bundled backgrounds (shaurma1: green in Wails/Electron, blue with a
/// triangle filter).
fn canvas_downscale(img: &DynamicImage, n: u32) -> Vec<[u8; 4]> {
    let src = img.to_rgba8();
    let (w, h) = src.dimensions();
    let at = |i: u32, len: u32| (((i as f64 + 0.5) * len as f64 / n as f64) as u32).min(len - 1);
    let mut out = Vec::with_capacity((n * n) as usize);
    for ty in 0..n {
        let y = at(ty, h);
        for tx in 0..n {
            out.push(src.get_pixel(at(tx, w), y).0);
        }
    }
    out
}

pub fn analyze_image(img: &DynamicImage) -> Option<ImageTheme> {
    const N: u32 = 64;
    let small = canvas_downscale(img, N);
    // Insertion-ordered like the JS Map: ties keep the first bin encountered.
    let mut index: std::collections::HashMap<u32, usize> = Default::default();
    let mut bins: Vec<[f64; 4]> = Vec::new();
    let mut lum_sum = 0.0;
    let mut n = 0.0;
    for p in &small {
        let [r, g, b, a] = *p;
        if a < 125 {
            continue;
        }
        lum_sum += (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64) / 255.0;
        n += 1.0;
        if r > 250 && g > 250 && b > 250 {
            continue;
        }
        let key = ((r as u32 >> 3) << 10) | ((g as u32 >> 3) << 5) | (b as u32 >> 3);
        let i = *index.entry(key).or_insert_with(|| {
            bins.push([0.0; 4]);
            bins.len() - 1
        });
        let bin = &mut bins[i];
        bin[0] += r as f64;
        bin[1] += g as f64;
        bin[2] += b as f64;
        bin[3] += 1.0;
    }
    if n == 0.0 {
        return None;
    }
    let mut best: Option<[f64; 4]> = None;
    let mut best_score = 0.0;
    let mut dominant: Option<[f64; 4]> = None;
    for bin in &bins {
        if dominant.map(|d| bin[3] > d[3]).unwrap_or(true) {
            dominant = Some(*bin);
        }
        let (r, g, b) = (bin[0] / bin[3], bin[1] / bin[3], bin[2] / bin[3]);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max - min < 22.0 || max < 28.0 {
            continue;
        }
        let score = bin[3] * (0.35 + (max - min) / 255.0);
        if score > best_score {
            best_score = score;
            best = Some(*bin);
        }
    }
    if best.map(|b| b[3] < n * 0.01).unwrap_or(true) {
        best = dominant;
    }
    let best = best?;
    let base: Rgb = [best[0] / best[3], best[1] / best[3], best[2] / best[3]];
    let white = lum_sum / n < 0.55;
    let [h0, s0, l0] = rgb_to_hsl(base[0], base[1], base[2]);
    let mut h = if (200.0..=250.0).contains(&h0) {
        h0 - 10.0
    } else if h0 > 250.0 && h0 < 320.0 {
        h0 - 20.0
    } else {
        (h0 + 25.0) % 360.0
    };
    let s = (s0 + 0.22 + if l0 < 0.5 { 0.05 } else { -0.05 }).min(0.88).min(1.0) + 0.28;
    let s = s.min(1.0);
    let mut l = (l0.max(0.48).min(0.68) + 0.03).min(0.9);
    if h0 > 40.0 && h0 < 65.0 && l0 > 0.7 {
        l -= 0.09;
        h = (h0 + 20.0) % 360.0;
    }
    let text: Rgb = if white { [255.0; 3] } else { [0.0; 3] };
    let mut i = 0;
    while i < 5 && contrast(hsl_to_rgb(h, s, l), text) < 4.5 {
        l += if white { -0.035 } else { 0.035 };
        i += 1;
    }
    if contrast(hsl_to_rgb(h, s, l), base) < 2.5 {
        l = (l + 0.08).min(0.9);
    }
    Some(ImageTheme { h: h.round(), s, l, white })
}

pub fn accent_for(theme: &ImageTheme, dark: bool) -> Rgb {
    let text: Rgb = if dark { [255.0; 3] } else { [0.0; 3] };
    let mut l = theme.l;
    let mut rgb = hsl_to_rgb(theme.h, theme.s, l);
    let mut i = 0;
    while i < 40 && contrast(rgb, text) < 4.5 {
        l = (l + if dark { -0.02 } else { 0.02 }).clamp(0.05, 0.95);
        rgb = hsl_to_rgb(theme.h, theme.s, l);
        i += 1;
    }
    rgb
}

/// Black or white, whichever reads better on the colour.
pub fn on_color(c: Rgb) -> Rgb {
    if contrast(c, [255.0; 3]) >= contrast(c, [0.0; 3]) {
        [255.0; 3]
    } else {
        [0.0; 3]
    }
}

/// The URL inside a CSS `url(...)` value.
pub fn extract_url(css: &str) -> Option<String> {
    let re = regex::Regex::new(r#"^url\(['"]?(.*?)['"]?\)$"#).ok()?;
    re.captures(css.trim()).map(|m| m[1].to_string())
}

/// Loads the picture behind a CSS background value.
pub async fn load_background(css: &str) -> Option<DynamicImage> {
    let url = extract_url(css).unwrap_or_else(|| css.trim().to_string());
    let url = url.replace("%27", "'");
    if let Some(name) = url.strip_prefix("/images/") {
        let name = name.split(['?', '#']).next().unwrap_or(name);
        return assets::bundled_image(name).and_then(|b| image::load_from_memory(b).ok());
    }
    let url = match url.find("/user-images/") {
        Some(i) => url[i..].to_string(),
        None => url,
    };
    if let Some(name) = url.strip_prefix("/user-images/") {
        if name.contains('/') || name.contains('\\') {
            return None;
        }
        let bytes = std::fs::read(paths::custom_images_dir().join(name)).ok()?;
        return image::load_from_memory(&bytes).ok();
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        // Random-image hosts return a new picture per request: show the one
        // fetched when this background was picked (bgCache.ts in the web shells).
        if let Some(img) = cached_background(css) {
            return Some(img);
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (Prizrak-Box)")
            .build()
            .ok()?;
        let resp = client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?;
        if bytes.len() > 15 << 20 {
            return None;
        }
        let img = image::load_from_memory(&bytes).ok()?;
        store_background_cache(css, &bytes);
        return Some(img);
    }
    if url.starts_with("data:") {
        let (_, b64) = url.split_once(";base64,")?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        return image::load_from_memory(&bytes).ok();
    }
    None
}

/// One-slot cache of the last remote background, keyed by the exact stored
/// value (random themes carry `&date=<pick time>`, so a new pick is a new
/// key): `<UserConfigDir>/prizrak-box/bg-cache.{key,img}`.
fn background_cache_files() -> (std::path::PathBuf, std::path::PathBuf) {
    (paths::flag_file("bg-cache.key"), paths::flag_file("bg-cache.img"))
}

fn cached_background(css: &str) -> Option<DynamicImage> {
    let (key, file) = background_cache_files();
    if std::fs::read_to_string(&key).map(|k| k == css).unwrap_or(false) {
        if let Some(img) = std::fs::read(&file).ok().and_then(|b| image::load_from_memory(&b).ok()) {
            return Some(img);
        }
    }
    electron_cached_background(css)
}

/// Electron (1.0.20) kept the same cache as `userData/px-bg-cache.json`
/// (`{forBg, dataUrl}`), so an upgrade keeps the picture the user had.
fn electron_cached_background(css: &str) -> Option<DynamicImage> {
    let file = dirs::config_dir()?.join("Prizrak-Box").join("px-bg-cache.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    if v.get("forBg")?.as_str()? != css {
        return None;
    }
    let (_, b64) = v.get("dataUrl")?.as_str()?.split_once(";base64,")?;
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    store_background_cache(css, &bytes);
    Some(img)
}

fn store_background_cache(css: &str, bytes: &[u8]) {
    let (key, file) = background_cache_files();
    if let Some(dir) = key.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Picture first, key last: a crash in between leaves a key that does not
    // match, never a key pointing at another picture.
    let _ = std::fs::remove_file(&key);
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &file).is_ok() {
        let _ = std::fs::write(&key, css);
    }
}

/// Display copy (≤ 1920 px) and the blurred backdrop copy.
pub fn prepare(img: &DynamicImage, blur_px: f64) -> (RgbaImage, Option<RgbaImage>) {
    let max_dim = 1920;
    let display = if img.width() > max_dim || img.height() > max_dim {
        img.resize(max_dim, max_dim, imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let display = display.to_rgba8();
    let blurred = if blur_px >= 2.0 {
        // CSS blur(N px) is a gaussian with σ = N on screen pixels. Work at
        // quarter resolution (σ / 4) — the result is smooth, so upscaling it
        // back in the renderer is invisible.
        let w = (display.width() / 4).max(1);
        let h = (display.height() / 4).max(1);
        let small = imageops::resize(&display, w, h, imageops::FilterType::Triangle);
        Some(imageops::blur(&small, (blur_px / 4.0) as f32))
    } else {
        None
    };
    (display, blurred)
}

pub fn to_slint(img: &RgbaImage) -> slint::Image {
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height());
    slint::Image::from_rgba8(buf)
}

/// The values of `useEffectiveTheme()`.
pub struct Effective {
    pub profile: Option<ProfileTheme>,
    pub pref: String,
    pub use_image: bool,
    pub background: String,
    pub trans: f64,
    pub blur: f64,
    pub dim: f64,
    pub fixed_accent: Option<String>,
}

pub fn effective(menu: &MenuState) -> Effective {
    let profile = if menu.use_profile_theme { menu.profile_theme.clone() } else { None };
    let p = profile.clone().unwrap_or_default();
    let image = p.image.clone().filter(|s| !s.is_empty());
    Effective {
        pref: p.mode.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| menu.theme_pref.clone()),
        use_image: image.is_some() || menu.use_bg_image,
        background: image
            .map(|i| format!("url('{}')", i.replace('\'', "%27")))
            .unwrap_or_else(|| menu.background.clone()),
        trans: p.transparency.unwrap_or(menu.ui_trans),
        blur: p.blur.unwrap_or(menu.ui_blur),
        dim: p.dim.unwrap_or(menu.bg_dim),
        fixed_accent: p.accent.clone().filter(|a| !a.is_empty() && a != "auto"),
        profile,
    }
}

/// OS light/dark preference (prefers-color-scheme).
pub fn system_dark() -> bool {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("dark"))
            .unwrap_or(false);
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("reg");
        cmd.args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "AppsUseLightTheme",
        ]);
        crate::core::hide_window(&mut cmd);
        return cmd
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("0x0"))
            .unwrap_or(true);
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if out.contains("dark") {
            return true;
        }
        if out.contains("light") || out.contains("default") {
            let gtk = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
                .unwrap_or_default();
            return gtk.contains("dark");
        }
        true
    }
}

pub fn rgb(c: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(c[0].round() as u8, c[1].round() as u8, c[2].round() as u8)
}

pub fn rgba(r: u8, g: u8, b: u8, a: f64) -> slint::Color {
    slint::Color::from_argb_u8((a.clamp(0.0, 1.0) * 255.0).round() as u8, r, g, b)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn hue(name: &str) -> f64 {
        let img = image::load_from_memory(assets::bundled_image(name).unwrap()).unwrap();
        analyze_image(&img).unwrap().h
    }

    /// Hues as WKWebView computes them with util/theme.ts (Wails, macOS).
    #[test]
    fn accent_matches_webkit() {
        for (name, webkit) in [("shaurma1.jpg", 146.0), ("harbor.jpg", 206.0), ("default.jpg", 232.0), ("woman1.jpg", 232.0), ("gradient5.png", 235.0)] {
            let h = hue(name);
            let d = (h - webkit).abs().min(360.0 - (h - webkit).abs());
            assert!(d <= 8.0, "{name}: {h} vs WebKit {webkit}");
        }
    }
}
