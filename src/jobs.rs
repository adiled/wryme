use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

struct Job {
    running: bool,
    partial: String,
    output: String,
    delivered: bool,
}

static REGISTRY: std::sync::LazyLock<Mutex<HashMap<u64, Job>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

const JOB_TIMEOUT_SECS: u64 = 120;
const JOB_OUTPUT_CAP: usize = 24_000;

pub struct Handle {
    pub id: u64,
    pub done: oneshot::Receiver<String>,
}

pub fn spawn(command: String) -> Handle {
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    {
        let mut r = REGISTRY.lock().unwrap();
        r.insert(
            id,
            Job {
                running: true,
                partial: String::new(),
                output: String::new(),
                delivered: false,
            },
        );
    }
    tokio::spawn(async move {
        let output = match tokio::time::timeout(
            Duration::from_secs(JOB_TIMEOUT_SECS),
            run_command(&command, id),
        )
        .await
        {
            Ok(out) => out,
            Err(_) => format!("(timed out after {}s)", JOB_TIMEOUT_SECS),
        };
        {
            let mut r = REGISTRY.lock().unwrap();
            if let Some(job) = r.get_mut(&id) {
                job.running = false;
                job.output = output.clone();
            }
        }
        let _ = tx.send(output);
    });
    Handle { id, done: rx }
}

pub struct JobStatus {
    pub running: bool,
    pub output: String,
}

pub fn poll(id: u64) -> Option<JobStatus> {
    let r = REGISTRY.lock().unwrap();
    r.get(&id).map(|j| JobStatus {
        running: j.running,
        output: if j.running {
            j.partial.clone()
        } else {
            j.output.clone()
        },
    })
}

pub fn mark_delivered(id: u64) {
    let mut r = REGISTRY.lock().unwrap();
    if let Some(job) = r.get_mut(&id) {
        job.delivered = true;
    }
}

pub fn has_due() -> bool {
    let r = REGISTRY.lock().unwrap();
    r.values().any(|j| !j.running && !j.delivered)
}

pub fn claim_due() -> Vec<(u64, String)> {
    let mut r = REGISTRY.lock().unwrap();
    let done: Vec<u64> = r
        .iter()
        .filter(|(_, j)| !j.running)
        .map(|(id, _)| *id)
        .collect();
    let mut due = Vec::new();
    for id in done {
        let job = r.remove(&id).unwrap();
        if !job.delivered {
            due.push((id, job.output));
        }
    }
    due
}

async fn run_command(command: &str, id: u64) -> String {
    let shell = crate::shell_env::shell();
    let mut child = match Command::new(shell)
        .arg("-l")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("could not start shell: {}", e),
    };
    let mut stdout = match child.stdout.take() {
        Some(o) => o,
        None => return "could not capture stdout".to_string(),
    };
    let mut stderr = match child.stderr.take() {
        Some(e) => e,
        None => return "could not capture stderr".to_string(),
    };

    let mut out_buf = [0u8; 8192];
    let mut err_buf = [0u8; 8192];
    let mut out_open = true;
    let mut err_open = true;
    loop {
        if !out_open && !err_open {
            break;
        }
        tokio::select! {
            biased;
            n = stdout.read(&mut out_buf), if out_open => {
                match n {
                    Ok(0) => out_open = false,
                    Ok(n) => append_partial(id, &out_buf[..n]),
                    Err(_) => out_open = false,
                }
            }
            n = stderr.read(&mut err_buf), if err_open => {
                match n {
                    Ok(0) => err_open = false,
                    Ok(n) => append_partial(id, &err_buf[..n]),
                    Err(_) => err_open = false,
                }
            }
        }
    }
    let code = match child.wait().await {
        Ok(st) => st.code(),
        Err(_) => None,
    };

    let r = REGISTRY.lock().unwrap();
    let partial = r.get(&id).map(|j| j.partial.clone()).unwrap_or_default();
    drop(r);
    let mut s = partial;
    if s.trim().is_empty() {
        s = format!("(no output) exit code: {}", code.unwrap_or(-1));
    } else if code != Some(0) {
        s.push_str(&format!("\nexit code: {}", code.unwrap_or(-1)));
    }
    crate::api::truncate(&s, JOB_OUTPUT_CAP)
}

fn append_partial(id: u64, chunk: &[u8]) {
    let s = String::from_utf8_lossy(chunk).into_owned();
    let mut r = REGISTRY.lock().unwrap();
    if let Some(job) = r.get_mut(&id)
        && job.partial.len() < JOB_OUTPUT_CAP
    {
        job.partial.push_str(&s);
    }
}

#[allow(clippy::await_holding_lock)]
#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn spawn_fast_job_delivers_output() {
        let _g = TEST_LOCK.lock().unwrap();
        let h = spawn("echo hi".to_string());
        let out = tokio::time::timeout(Duration::from_secs(10), h.done)
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(out.contains("hi"));
    }

    #[tokio::test]
    async fn poll_reports_running_with_partial() {
        let _g = TEST_LOCK.lock().unwrap();
        let h = spawn("echo one; sleep 0.1; echo two".to_string());
        let st = poll(h.id).expect("job exists");
        assert!(st.running);
        let out = tokio::time::timeout(Duration::from_secs(10), h.done)
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(out.contains("two"));
    }

    #[tokio::test]
    async fn claim_due_returns_undelivered_and_removes() {
        let _g = TEST_LOCK.lock().unwrap();
        let h = spawn("echo done".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(10), h.done)
            .await
            .expect("timed out")
            .expect("channel closed");
        let due = claim_due();
        assert!(
            due.iter()
                .any(|(id, out)| *id == h.id && out.contains("done"))
        );
        assert!(poll(h.id).is_none());
    }

    #[tokio::test]
    async fn mark_delivered_suppresses_claim() {
        let _g = TEST_LOCK.lock().unwrap();
        let h = spawn("echo done".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(10), h.done)
            .await
            .expect("timed out")
            .expect("channel closed");
        mark_delivered(h.id);
        let due = claim_due();
        assert!(!due.iter().any(|(id, _)| *id == h.id));
    }

    #[tokio::test]
    async fn poll_unknown_job_is_none() {
        let _g = TEST_LOCK.lock().unwrap();
        assert!(poll(9999).is_none());
    }

    #[tokio::test]
    async fn async_job_outlives_wait_then_is_claimed() {
        let _g = TEST_LOCK.lock().unwrap();
        let h = spawn("sleep 0.3; echo late".to_string());
        let st = poll(h.id).expect("job exists");
        assert!(st.running);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(has_due());
        let due = claim_due();
        assert!(
            due.iter()
                .any(|(id, out)| *id == h.id && out.contains("late"))
        );
    }
}
