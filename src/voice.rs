use std::process::{Child, Command};

pub const DEFAULT_MAC_VOICE: &str = "Tara";
pub const DEFAULT_MAC_RATE_WPM: &str = "260";

fn has_bin(name: &str) -> bool {
    std::env::var_os("PATH").map_or(false, |paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .any(|p| p.is_file())
    })
}

pub fn available() -> bool {
    if cfg!(target_os = "macos") {
        has_bin("say")
    } else {
        has_bin("spd-say")
    }
}

fn spawn_say(body: &str, voice: Option<&str>) -> Option<Child> {
    if body.trim().is_empty() {
        return None;
    }
    if cfg!(target_os = "macos") {
        return None;
    }
    let mut c = Command::new("spd-say");
    if let Some(v) = voice {
        c.arg("-v").arg(v);
    }
    c.arg(body).spawn().ok()
}

static SPEECH_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

type Cur = std::sync::Arc<std::sync::Mutex<Option<Child>>>;

fn synth_file(body: &str, voice: Option<&str>, cur: &Cur) -> Option<std::path::PathBuf> {
    let n = SPEECH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("wryme-say-{}-{}.aiff", std::process::id(), n));
    let mut c = Command::new("say");
    match voice {
        Some(v) => {
            c.arg("-v").arg(v);
        }
        None => {
            c.arg("-v").arg(DEFAULT_MAC_VOICE);
            c.arg("-r").arg(DEFAULT_MAC_RATE_WPM);
        }
    }
    let spawned = c.arg("-o").arg(&path).arg(body).spawn();
    let Ok(child) = spawned else {
        let _ = std::fs::remove_file(&path);
        return None;
    };
    if let Ok(mut guard) = cur.lock() {
        *guard = Some(child);
    }
    let ok = wait_releasable(cur);
    if ok && path.is_file() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// Wait for the current child without holding the lock: polls so Stop
/// can take + kill mid-speech. False when the child was taken (=killed)
/// or failed.
fn wait_releasable(cur: &Cur) -> bool {
    loop {
        let done = if let Ok(mut guard) = cur.lock() {
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(status.success()),
                    Ok(None) => None,
                    Err(_) => Some(false),
                },
                None => Some(false),
            }
        } else {
            None
        };
        match done {
            Some(ok) => return ok,
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

fn play_wait(path: &std::path::Path, cur: &Cur) {
    if let Ok(child) = Command::new("afplay").arg(path).spawn() {
        if let Ok(mut guard) = cur.lock() {
            *guard = Some(child);
        }
        wait_releasable(cur);
    }
    let _ = std::fs::remove_file(path);
}

/// Split streamed text into speakable sentences. Returns complete
/// sentences, keeping the unfinished tail buffered by the caller.
pub fn split_sentences(buffer: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    let mut end = 0usize;
    let chars: Vec<char> = buffer.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if (c == '.' || c == '!' || c == '?')
            && (i + 1 == chars.len() || chars[i + 1].is_whitespace())
        {
            end = buffer
                .char_indices()
                .nth(i + 1)
                .map(|(b, _)| b)
                .unwrap_or(buffer.len());
            break;
        }
        i += 1;
    }
    if end > 0 {
        let head = buffer[..end].trim().to_string();
        buffer.drain(..end);
        if !head.is_empty() {
            out.push(head);
        }
        out.extend(split_sentences(buffer));
    }
    out
}

enum SpeakCmd {
    Say(String),
    Flush,
    Stop,
}

/// Sequential background speaker. Sentences queue up and play in order;
/// Stop kills the current voice and drops the queue. macOS renders each
/// speech to a temp file and plays it with afplay: `say` straight to the
/// device clips the tail, the file round-trip does not.
pub struct Speaker {
    tx: Option<std::sync::mpsc::Sender<SpeakCmd>>,
    current: std::sync::Arc<std::sync::Mutex<Option<Child>>>,
    queued: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub name: Option<String>,
}

impl Speaker {
    pub fn new(voice: Option<String>) -> Self {
        let current = std::sync::Arc::new(std::sync::Mutex::new(None::<Child>));
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cur = current.clone();
        let cnt = queued.clone();
        let thread_voice = voice.clone();
        let (tx, rx) = std::sync::mpsc::channel::<SpeakCmd>();
        std::thread::spawn(move || {
            let mut pending: Vec<String> = Vec::new();
            // One speech, fully played. Batches two sentences where told
            // to; afplay is the killable handle, temp file is scrubbed.
            let speak_now = |text: String, cur: &Cur| {
                if text.trim().is_empty() {
                    return;
                }
                if cfg!(target_os = "macos") {
                    if let Some(path) = synth_file(&text, thread_voice.as_deref(), cur) {
                        play_wait(&path, cur);
                    }
                    return;
                }
                if let Some(child) = spawn_say(&text, thread_voice.as_deref()) {
                    if let Ok(mut guard) = cur.lock() {
                        *guard = Some(child);
                    }
                    if let Ok(mut guard) = cur.lock() {
                        if let Some(mut child) = guard.take() {
                            let _ = child.wait();
                        }
                    }
                }
            };
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    SpeakCmd::Stop => {
                        pending.clear();
                        cnt.store(0, std::sync::atomic::Ordering::Relaxed);
                        if let Ok(mut guard) = cur.lock() {
                            if let Some(mut child) = guard.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                        }
                        while rx.try_recv().is_ok() {}
                    }
                    SpeakCmd::Say(text) => {
                        pending.push(text);
                        while pending.len() >= 2 {
                            let pair = format!("{} {}", pending.remove(0), pending.remove(0));
                            cnt.fetch_sub(2, std::sync::atomic::Ordering::Relaxed);
                            speak_now(pair, &cur);
                        }
                    }
                    SpeakCmd::Flush => {
                        if !pending.is_empty() {
                            let rest = pending.join(" ");
                            cnt.store(0, std::sync::atomic::Ordering::Relaxed);
                            pending.clear();
                            speak_now(rest, &cur);
                        }
                    }
                }
            }
        });
        Self {
            tx: Some(tx),
            current,
            queued,
            name: voice,
        }
    }

    pub fn say(&self, text: String) {
        if self.tx.is_some() {
            self.queued
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(SpeakCmd::Say(text));
        }
    }

    pub fn is_active(&self) -> bool {
        if self.queued.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            return true;
        }
        self.current.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn flush(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(SpeakCmd::Flush);
        }
    }

    pub fn stop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(SpeakCmd::Stop);
        }
        if let Ok(mut guard) = self.current.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.stop();
        self.tx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_speaks_nothing() {
        let mut buf = String::from("no sentence end here");
        assert!(split_sentences(&mut buf).is_empty());
        assert_eq!(buf, "no sentence end here");
    }

    #[test]
    fn sentences_split_on_terminators() {
        let mut buf = String::from("The kettle is warm. Second line! Is it? Yes");
        let out = split_sentences(&mut buf);
        assert_eq!(out, vec!["The kettle is warm.", "Second line!", "Is it?"]);
        assert_eq!(buf, " Yes");
    }

    #[test]
    fn backend_detection_finds_path_bins() {
        assert!(has_bin("sh"));
        assert!(!has_bin("definitely-not-a-real-binary-xyz"));
    }
}
