use dashmap::DashMap;
use std::time::{Duration, Instant};

pub struct HostRateLimiter {
    default_rps: Option<f64>,
    state: DashMap<String, Instant>,
}

impl HostRateLimiter {
    pub fn new(default_rps: Option<f64>) -> Self {
        Self {
            default_rps,
            state: DashMap::new(),
        }
    }

    pub async fn acquire(&self, host: &str) {
        let Some(rps) = self.default_rps else { return };
        if rps <= 0.0 {
            return;
        }
        let interval = Duration::from_secs_f64(1.0 / rps);
        loop {
            let now = Instant::now();
            let delay;
            {
                let mut entry = self
                    .state
                    .entry(host.to_string())
                    .or_insert_with(|| now.checked_sub(interval).unwrap_or(now));
                let next = *entry + interval;
                if next > now {
                    delay = next - now;
                } else {
                    *entry = now;
                    return;
                }
            }
            tokio::time::sleep(delay).await;
        }
    }
}
