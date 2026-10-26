//! Prizrak-Box desktop shell — Rust + Slint (no web engine).
//!
//! Replaces the Wails v3 shell (src-wails) and the Electron shell
//! (src-electron) while talking to the very same px backend (src-go) and
//! px-service (src-service). See doc/rust-slint-migration-analysis.md.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

slint::include_modules!();

mod api;
mod app;
mod assets;
mod autostart;
mod backdrop;
mod core;
mod deeplink;
mod fileicon;
mod hotkey;
mod i18n;
mod images;
mod migrate;
mod notify;
mod pages;
mod paths;
mod platform;
mod single_instance;
mod snapshot;
mod store;
mod theme;
mod tray;
mod tun;
mod util;
mod window;
mod ws;

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use slint::ComponentHandle;

/// Appends a line to the shell log (GUI builds have no console).
pub fn shell_log(msg: &str) {
    let path = paths::shell_log_file();
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [rs] {}", util::format_date_time(chrono::Local::now()), msg);
    }
}

/// The web shells' font stack is `Twemoji, -apple-system, 'Segoe UI', …`.
/// Slint's generic sans-serif fallback is Helvetica on macOS and Arial on
/// Windows; on Windows (where the bundled Twemoji is the primary font for
/// flags) every other glyph falls back to it. SLINT_FONT_PATH puts Segoe UI
/// into that fallback chain instead — must run before Slint loads fonts.
fn set_font_fallback() {
    #[cfg(windows)]
    {
        if std::env::var_os("SLINT_FONT_PATH").is_some() {
            return;
        }
        let dir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
        let files: Vec<std::path::PathBuf> = ["segoeui.ttf", "seguisb.ttf", "segoeuib.ttf", "segoeuil.ttf", "segoeuisl.ttf"]
            .iter()
            .map(|f| dir.join(f))
            .filter(|p| p.is_file())
            .collect();
        if let Ok(list) = std::env::join_paths(files) {
            std::env::set_var("SLINT_FONT_PATH", list);
        }
    }
}

fn main() {
    set_font_fallback();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    std::panic::set_hook(Box::new(|info| {
        shell_log(&format!("PANIC: {info}"));
        eprintln!("PANIC: {info}");
    }));

    let args: Vec<String> = std::env::args().skip(1).collect();
    let initial_link = deeplink::find(&args);

    // Second launch: hand argv (deep link) to the running instance and leave.
    let primary = single_instance::acquire(&args, |argv| {
        window::show();
        if let Some(link) = deeplink::find(&argv) {
            deeplink::handle(link);
        }
    });
    if !primary {
        return;
    }

    migrate::run();
    notify::init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(4)
        .build()
        .expect("tokio runtime");

    // Slint on winit; window attributes give macOS its hidden-inset title bar.
    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(window::attributes);
    if let Err(e) = selector.select() {
        shell_log(&format!("renderer selection failed, falling back: {e}"));
        let _ = slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .with_winit_window_attributes_hook(window::attributes)
            .select();
    }

    #[cfg(target_os = "macos")]
    platform::register_url_handler();

    let ui = AppWindow::new().expect("create window");
    let core = Arc::new(core::Core::new());
    let tun = Arc::new(tun::Tun::new(core.clone()));
    app::init(&ui, rt.handle().clone(), core.clone(), tun.clone());
    #[cfg(unix)]
    app::watch_termination();

    // Closing the window (Alt+F4, the taskbar's Close window) hides it to the
    // tray like the title-bar close button; on Windows / Linux without a tray
    // host it minimises instead (window::close_to_tray).
    ui.window().on_close_requested(|| {
        if cfg!(target_os = "macos") {
            slint::CloseRequestResponse::HideWindow
        } else {
            window::close_to_tray();
            slint::CloseRequestResponse::KeepWindowShown
        }
    });

    // Start px in the background (through the TUN service when available).
    {
        let core = core.clone();
        let tun = tun.clone();
        std::thread::spawn(move || {
            if let Err(e) = core.ensure_callback_server() {
                app::on_backend_error(format!("callback server: {e}"));
                return;
            }
            match tun.start_backend() {
                Ok(info) => app::on_backend_ready(info),
                Err(e) => app::on_backend_error(e.to_string()),
            }
        });
    }

    // Once the event loop runs: tray, hotkeys, initial reveal, deep link.
    let link = initial_link.clone();
    slint::Timer::single_shot(Duration::from_millis(1), move || {
        #[cfg(not(target_os = "linux"))]
        tray::create();
        #[cfg(target_os = "linux")]
        tray::spawn_linux();
        hotkey::init_on_main();
        let start_hidden = paths::start_minimized() && link.is_none() && tray::host_available();
        if !start_hidden {
            window::show();
        }
        window::polish_native();
        if let Some(l) = link {
            deeplink::handle(l);
        }
    });

    snapshot::schedule(&ui);

    // 1 s ticker: runtime clock, maximise state.
    let weak = ui.as_weak();
    let ticker = slint::Timer::default();
    ticker.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(w) = weak.upgrade() {
            app::tick_runtime(&w);
            window::poll_state(&w);
            pages::tick(&w);
        }
    });

    if let Err(e) = slint::run_event_loop_until_quit() {
        shell_log(&format!("event loop: {e}"));
    }
    drop(ticker);
    core.request_exit();
    core.kill_px();
    rt.shutdown_timeout(Duration::from_secs(1));
}
