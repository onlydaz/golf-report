//! In-memory stderr filtering only; never participates in PLC/DB decisions.
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
};

/// Opt in before starting the process. No logging dependency or TOML change.
pub(super) fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GOLF_REPORT_PLC_DEBUG").is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "debug" | "trace"
            )
        })
    })
}

type TcpSource = (SocketAddr, Option<IpAddr>);
fn failed_tcp_sources() -> &'static Mutex<HashSet<TcpSource>> {
    static FAILED: OnceLock<Mutex<HashSet<TcpSource>>> = OnceLock::new();
    FAILED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(super) fn note_tcp_failure(endpoint: SocketAddr, source: Option<IpAddr>) {
    if let Ok(mut failed) = failed_tcp_sources().lock() {
        failed.insert((endpoint, source));
    }
}

/// Only a failed open for this endpoint/source makes the next open a recovery.
/// A new ephemeral port by itself never counts as a reconnect.
pub(super) fn take_tcp_failure(endpoint: SocketAddr, source: Option<IpAddr>) -> bool {
    failed_tcp_sources()
        .lock()
        .map(|mut failed| failed.remove(&(endpoint, source)))
        .unwrap_or(true)
}

/// One last value per fixed event name, not an ever-growing publication history.
#[derive(Default)]
pub(super) struct ChangedLogs {
    previous: HashMap<&'static str, String>,
}

impl ChangedLogs {
    pub(super) fn should_emit(&mut self, event: &'static str, value: String) -> bool {
        let changed = self.previous.get(event) != Some(&value);
        if changed {
            self.previous.insert(event, value);
        }
        changed || debug_enabled()
    }

    pub(super) fn forget(&mut self, event: &'static str) -> bool {
        self.previous.remove(event).is_some()
    }

    pub(super) fn clear(&mut self) {
        self.previous.clear();
    }
}
