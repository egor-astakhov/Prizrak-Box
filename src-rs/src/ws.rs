//! px WebSockets (`/logs`, `/connections`, `/profile/order`) — port of
//! util/ws.ts plus the reconnect-after-restart logic of backendConn.ts.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::app::{self, ctx};

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub id: u64,
    pub time: String,
    pub kind: String,
    pub payload: String,
}

pub static LOGS: Mutex<VecDeque<LogEntry>> = Mutex::new(VecDeque::new());
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);
static LOG_GEN: AtomicU64 = AtomicU64::new(0);
static CONN_GEN: AtomicU64 = AtomicU64::new(0);
pub static LOG_LEVEL: Mutex<String> = Mutex::new(String::new());
static ORDER_TX: Mutex<Option<mpsc::UnboundedSender<String>>> = Mutex::new(None);
static CONN_WANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs a websocket until `gen` changes; reconnects on drops.
async fn run<F>(path: String, gen: &'static AtomicU64, my: u64, mut on_msg: F, mut outgoing: Option<mpsc::UnboundedReceiver<String>>)
where
    F: FnMut(String) + Send,
{
    while gen.load(Ordering::SeqCst) == my {
        let url = ctx().api.ws_url(&path);
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((stream, _)) => {
                let (mut sink, mut src) = stream.split();
                loop {
                    if gen.load(Ordering::SeqCst) != my {
                        let _ = sink.close().await;
                        return;
                    }
                    let out_fut = async {
                        match outgoing.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    };
                    tokio::select! {
                        msg = src.next() => match msg {
                            Some(Ok(Message::Text(t))) => on_msg(t.to_string()),
                            Some(Ok(Message::Binary(b))) => on_msg(String::from_utf8_lossy(&b).to_string()),
                            Some(Ok(_)) => {}
                            _ => break,
                        },
                        out = out_fut => {
                            if let Some(s) = out {
                                if sink.send(Message::Text(s.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
            }
            Err(e) => log::debug!("ws {path}: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub fn start_logs() {
    let my = LOG_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let level = LOG_LEVEL.lock().clone();
    let path = if level.is_empty() { "/logs".to_string() } else { format!("/logs?level={}", crate::api::enc(&level)) };
    app::spawn(async move {
        run(
            path,
            &LOG_GEN,
            my,
            |text| {
                let Ok(v) = serde_json::from_str::<Value>(&text) else { return };
                let entry = LogEntry {
                    id: LOG_SEQ.fetch_add(1, Ordering::SeqCst),
                    time: crate::util::format_date_time(chrono::Local::now()),
                    kind: v.get("type").and_then(Value::as_str).unwrap_or("info").to_uppercase(),
                    payload: v.get("payload").and_then(Value::as_str).unwrap_or_default().to_string(),
                };
                {
                    let mut l = LOGS.lock();
                    l.push_back(entry);
                    while l.len() > 1000 {
                        l.pop_front();
                    }
                }
                crate::pages::on_log();
            },
            None,
        )
        .await;
    });
}

pub fn set_log_level(level: &str) {
    *LOG_LEVEL.lock() = level.to_string();
    LOGS.lock().clear();
    start_logs();
}

/// Traffic is only consumed through /connections in this UI.
pub fn start_traffic() {}

pub fn start_connections() {
    CONN_WANTED.store(true, Ordering::SeqCst);
    let my = CONN_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    app::spawn(async move {
        run(
            "/connections".into(),
            &CONN_GEN,
            my,
            |text| {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    crate::pages::on_connections(v);
                }
            },
            None,
        )
        .await;
    });
}

pub fn stop_connections() {
    CONN_WANTED.store(false, Ordering::SeqCst);
    CONN_GEN.fetch_add(1, Ordering::SeqCst);
}

static ORDER_GEN: AtomicU64 = AtomicU64::new(0);

/// `/profile/order` socket used by drag-to-reorder on the Profiles page.
pub fn send_order(payload: String) {
    let tx = ORDER_TX.lock().clone();
    if let Some(tx) = tx {
        if tx.send(payload.clone()).is_ok() {
            return;
        }
    }
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = tx.send(payload);
    *ORDER_TX.lock() = Some(tx);
    let my = ORDER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    app::spawn(async move {
        run("/profile/order".into(), &ORDER_GEN, my, |_| {}, Some(rx)).await;
        *ORDER_TX.lock() = None;
    });
}

/// After px restarted on another port/secret.
pub fn reconnect_all() {
    start_logs();
    if CONN_WANTED.load(Ordering::SeqCst) {
        start_connections();
    }
    ORDER_GEN.fetch_add(1, Ordering::SeqCst);
    *ORDER_TX.lock() = None;
}

pub fn _unused(_: Arc<()>) {}
