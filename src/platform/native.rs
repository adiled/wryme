//! Native seam: tokio timers, tokio tasks, std instant.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn sleep(d: Duration) -> impl Future<Output = ()> {
    tokio::time::sleep(d)
}

pub async fn timeout<F: Future>(d: Duration, f: F) -> Option<F::Output> {
    tokio::time::timeout(d, f).await.ok()
}

pub struct Task(tokio::task::JoinHandle<()>);

impl Task {
    pub fn abort(self) {
        self.0.abort();
    }
}

pub fn spawn<F>(fut: F) -> Task
where
    F: Future<Output = ()> + Send + 'static,
{
    Task(tokio::spawn(fut))
}

#[derive(Debug, Clone, Copy)]
pub struct Instant(std::time::Instant);

impl Instant {
    pub fn now() -> Self {
        Self(std::time::Instant::now())
    }

    pub fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

pub fn unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
