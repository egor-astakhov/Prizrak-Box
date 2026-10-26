//! Single-instance lock: a second launch hands its argv (deep links) to the
//! running instance over a local socket and exits; the first one shows its
//! window (OnSecondInstanceLaunch of the Wails shell).

use std::io::{BufRead, BufReader, Write};

use interprocess::local_socket::{prelude::*, GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream};

fn name() -> std::io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        "com.legiz-ru.prizrak-box.rs".to_ns_name::<GenericNamespaced>()
    } else {
        let p = std::env::temp_dir().join("com.legiz-ru.prizrak-box.sock");
        p.to_string_lossy().to_string().to_fs_name::<GenericFilePath>()
    }
}

/// Returns false when another instance is running (argv was forwarded).
pub fn acquire<F: Fn(Vec<String>) + Send + 'static>(args: &[String], on_second: F) -> bool {
    let Ok(n) = name() else { return true };
    if let Ok(mut s) = Stream::connect(n.clone()) {
        let payload = serde_json::to_string(args).unwrap_or_else(|_| "[]".into());
        let _ = s.write_all(payload.as_bytes());
        let _ = s.write_all(b"\n");
        let _ = s.flush();
        return false;
    }
    let listener = match ListenerOptions::new().name(n).reclaim_name(true).try_overwrite(true).create_sync() {
        Ok(l) => l,
        Err(e) => {
            log::warn!("single instance listener: {e}");
            return true;
        }
    };
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                let mut r = BufReader::new(conn);
                if r.read_line(&mut line).is_ok() {
                    let argv: Vec<String> = serde_json::from_str(line.trim()).unwrap_or_default();
                    on_second(argv);
                }
            }
        })
        .ok();
    true
}
