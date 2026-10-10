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

pub fn copy(text: &str) {
    #[cfg(target_os = "macos")]
    if pipe_to("pbcopy", text) {
        return;
    }
    #[cfg(target_os = "windows")]
    if pipe_to("clip", text) {
        return;
    }
    use crossterm::{clipboard::CopyToClipboard, execute};
    let _ = execute!(std::io::stdout(), CopyToClipboard::to_clipboard_from(text));
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pipe_to(program: &str, text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(program).stdin(Stdio::piped()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(text.as_bytes()).is_err()
    {
        return false;
    }
    child.wait().is_ok()
}
