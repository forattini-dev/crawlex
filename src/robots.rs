// robots.txt fetch/cache/evaluate per host.
// Uses `texting_robots` for RFC-compliant parsing. TODO: wire into Crawler.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use texting_robots::Robot;
use url::Url;

use crate::Result;

pub struct RobotsCache {
    ttl: Duration,
    inner: DashMap<String, (Instant, Arc<Option<Robot>>)>,
}

impl RobotsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: DashMap::new(),
        }
    }

    pub fn check(&self, url: &Url, user_agent: &str) -> Option<bool> {
        let host = url.host_str()?;
        let entry = self.inner.get(host)?;
        let (inserted, robot) = entry.value();
        if inserted.elapsed() > self.ttl {
            return None;
        }
        robot
            .as_ref()
            .as_ref()
            .map(|r| r.allowed(url.as_str()))
            .or(Some(true))
            .map(|allowed| {
                let _ = user_agent;
                allowed
            })
    }

    pub fn store(&self, host: &str, txt: Option<&str>, user_agent: &str) -> Result<()> {
        let robot = txt.and_then(|t| Robot::new(user_agent, t.as_bytes()).ok());
        self.inner
            .insert(host.to_string(), (Instant::now(), Arc::new(robot)));
        Ok(())
    }
}
