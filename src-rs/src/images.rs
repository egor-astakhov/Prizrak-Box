//! Remote / inline images (profile logos, proxy-group icons) with a small
//! in-memory cache. Accepts http(s), `data:` URLs, PNG/JPEG/WebP/ICO and SVG.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use image::RgbaImage;
use parking_lot::Mutex;

fn cache() -> &'static Mutex<HashMap<String, Option<RgbaImage>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<RgbaImage>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub fn decode(bytes: &[u8], size_hint: u32) -> Option<RgbaImage> {
    if let Ok(i) = image::load_from_memory(bytes) {
        let i = if i.width() > 256 || i.height() > 256 {
            i.resize(256, 256, image::imageops::FilterType::Triangle)
        } else {
            i
        };
        return Some(i.to_rgba8());
    }
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let sz = tree.size();
    let target = size_hint.max(16) as f32;
    let scale = (target / sz.width()).min(target / sz.height());
    let w = (sz.width() * scale).ceil().max(1.0) as u32;
    let h = (sz.height() * scale).ceil().max(1.0) as u32;
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    let mut data = pm.take();
    for px in data.chunks_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in px.iter_mut().take(3) {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(w, h, data)
}

pub async fn load_remote(url: &str) -> Option<RgbaImage> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    if let Some(v) = cache().lock().get(&url) {
        return v.clone();
    }
    let res = fetch(&url).await;
    cache().lock().insert(url, res.clone());
    res
}

async fn fetch(url: &str) -> Option<RgbaImage> {
    if url.starts_with("data:") {
        let (_, b64) = url.split_once(";base64,")?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
        return decode(&bytes, 128);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Prizrak-Box)")
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    decode(&bytes, 128)
}
