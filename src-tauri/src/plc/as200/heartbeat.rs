//! Freshness dùng đồng hồ monotonic; không suy reset phiên từ Heartbeat wrap.
use std::time::{Duration, Instant};

pub struct HeartbeatMonitor {
    timeout: Option<Duration>,
    last: Option<i16>,
    changed_at: Instant,
    seen_change: bool,
}
impl HeartbeatMonitor {
    pub fn new(timeout: Option<Duration>, now: Instant) -> Self {
        Self {
            timeout,
            last: None,
            changed_at: now,
            seen_change: false,
        }
    }
    pub fn observe(&mut self, value: i16, now: Instant) -> &'static str {
        if self.last != Some(value) {
            self.seen_change |= self.last.is_some();
            self.changed_at = now;
            self.last = Some(value);
        }
        self.health(now)
    }
    pub fn health(&self, now: Instant) -> &'static str {
        match self.timeout {
            None => "UNCONFIGURED",
            Some(timeout) if now.saturating_duration_since(self.changed_at) >= timeout => "STALE",
            Some(_) if self.seen_change => "RUNNING",
            _ => "OBSERVING",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_stalled_wrap_and_recovery() {
        let now = Instant::now();
        let mut monitor = HeartbeatMonitor::new(Some(Duration::from_secs(3)), now);
        assert_eq!(monitor.observe(32767, now), "OBSERVING");
        assert_eq!(monitor.observe(0, now + Duration::from_secs(1)), "RUNNING");
        assert_eq!(monitor.observe(0, now + Duration::from_secs(4)), "STALE");
        assert_eq!(monitor.observe(1, now + Duration::from_secs(5)), "RUNNING");
        assert_eq!(
            HeartbeatMonitor::new(None, now).observe(1, now),
            "UNCONFIGURED"
        );
    }
}
