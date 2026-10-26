//! Blurred scrim backdrop (`--scrim-blur: blur(4px)` of UiModal, UiNotice,
//! the loading toast and the drop / deep-link overlays).
//!
//! Slint draws no backdrop filters, so when an overlay opens it calls
//! `Backdrop.request()`; the window is then rendered off screen with every
//! overlay hidden (`capturing`), the picture is blurred and published, and the
//! overlay shows up on top of it.

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use image::{imageops, RgbaImage};
use slint::ComponentHandle;

use crate::app::ui;
use crate::{AppWindow, Backdrop, ThemeDialog};

static NEXT: AtomicI32 = AtomicI32::new(0);

/// CSS `blur(4px)` in logical pixels.
const BLUR_PX: f32 = 4.0;
/// The picture is blurred (and shown) at 1/DOWNSCALE of the window size.
const DOWNSCALE: u32 = 4;

pub fn wire(w: &AppWindow) {
    w.global::<Backdrop>().on_request(|| {
        let id = NEXT.fetch_add(1, Ordering::SeqCst) + 1;
        ui(move |w| capture(&w, id));
        id
    });
}

fn capture(w: &AppWindow, id: i32) {
    let bd = w.global::<Backdrop>();
    // Hide the requesting overlay and anything opened after it; dialogs that
    // were already open stay in the picture (and get blurred with it).
    bd.set_capture_id(id);
    bd.set_capturing(true);
    let shot = w.window().take_snapshot();
    bd.set_capturing(false);
    let scale = w.window().scale_factor();
    let image = shot.ok().and_then(|buf| {
        let img = RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())?;
        Some(crate::theme::to_slint(&blur(&img, scale)))
    });
    bd.set_image(image.unwrap_or_default());
    bd.set_image_id(id);
}

fn blur(img: &RgbaImage, scale: f32) -> RgbaImage {
    let w = (img.width() / DOWNSCALE).max(1);
    let h = (img.height() / DOWNSCALE).max(1);
    let small = imageops::resize(img, w, h, imageops::FilterType::Triangle);
    // Box-filter approximation of the gaussian: linear time, same look.
    imageops::fast_blur(&small, (BLUR_PX * scale / DOWNSCALE as f32).max(0.5))
}

/// The look behind an open theme dialog changed (background, mode, blur …):
/// re-render its backdrop once the changes settle, so the new background
/// shows through the scrim like the live backdrop-filter does.
pub fn refresh_soon() {
    thread_local! {
        static TIMER: slint::Timer = slint::Timer::default();
    }
    ui(|_| {
        TIMER.with(|t| {
            t.start(slint::TimerMode::SingleShot, Duration::from_millis(180), || {
                ui(|w| {
                    if w.global::<ThemeDialog>().get_open() {
                        let bd = w.global::<Backdrop>();
                        bd.set_refresh_tick(bd.get_refresh_tick() + 1);
                    }
                });
            });
        });
    });
}
