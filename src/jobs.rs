// Background shell jobs.
//
// A shell command that finishes within the 10s sync window returns its
// output straight to the model. Anything slower is parked here as an
// async job: it keeps running in the background, its output streams into
// the registry as it goes, and the model can either peek at progress
// (`<shell>_check`) or get the final result auto-delivered when done.
//
// This is what keeps the conversation moving: the turn never blocks on a
// long command, and a finished job is planted back into the model's
// context (as a check-call + result pair) without grandma having to ask.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;

/// One background job. `running` while the command is still going;
/// `partial` accumulates output as it streams (so `<shell>_check` can
/// show progress); `output` is the final result once done. `delivered`
/// marks that a finished result has already been shown to the model, so
/// the background auto-delivery won't plant it a second time.
struct Job {
    running: bool,
    partial: String,
    output: String,
    delivered: bool,
}

static REGISTRY: std::sync::LazyLock<Mutex<HashMap<u64, Job>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How long a background job may run before we kill it. Long, but not
/// infinite — no runaway processes.
const JOB_TIMEOUT_SECS: u64 = 120;
/// Cap on accumulated output we keep around for peeking.
const JOB_OUTPUT_CAP: usize = 24_000;

/// A spawned background job plus a receiver that resolves to its final
/// output when it finishes (the 10s sync path awaits this).
pub struct Handle {
    pub id: u64,
    pub done: oneshot::Receiver<String>,
}

/// Spawn a command as a background job, returning its id and a receiver
/// for the final output.
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

/// Current state of a job, for `<shell>_check`.
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

/// Mark a finished job as already shown to the model, so auto-delivery
/// won't plant it again.
pub fn mark_delivered(id: u64) {
    let mut r = REGISTRY.lock().unwrap();
    if let Some(job) = r.get_mut(&id) {
        job.delivered = true;
    }
}

/// True when a finished job is still waiting to be delivered to the
/// model. The main loop's idle tick uses this to fire a background turn.
pub fn has_due() -> bool {
    let r = REGISTRY.lock().unwrap();
    r.values().any(|j| !j.running && !j.delivered)
}

/// Remove every finished job and return the ones that were not yet shown
/// to the model. The protocol plants those as a check-call + result pair.
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

/// Run the command in the user's real login shell, streaming stdout and
/// stderr into the registry's `partial` as it arrives, then return the
/// final output (stdout + stderr + exit code).
async fn run_command(command: &str, id: u64) -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut child = match Command::new(&shell)
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

    // Read both streams concurrently so a chatty process never deadlocks
    // on a full pipe. We keep reading even past the cap (to drain), but
    // only buffer up to the cap.
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
    if let Some(job) = r.get_mut(&id) {
        if job.partial.len() < JOB_OUTPUT_CAP {
            job.partial.push_str(&s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The registry is global, so these tests must not race each other.
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
        // The job is running; poll should show it and eventually finish.
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
        assert!(due.iter().any(|(id, out)| *id == h.id && out.contains("done")));
        // claim_due removes every finished job, including ours.
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
        // A delivered job is never re-planted.
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
        // A job that runs longer than our 10s wait window: we never
        // await it, so it goes async and is delivered later via claim_due.
        let h = spawn("sleep 0.3; echo late".to_string());
        let st = poll(h.id).expect("job exists");
        assert!(st.running);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(has_due());
        let due = claim_due();
        assert!(due
            .iter()
            .any(|(id, out)| *id == h.id && out.contains("late")));
    }
}
