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
    let mut cmd = if cfg!(target_os = "macos") {
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
        c
    } else {
        let mut c = Command::new("spd-say");
        if let Some(v) = voice {
            c.arg("-v").arg(v);
        }
        c
    };
    cmd.arg(body).spawn().ok()
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

/// Sequential background speaker. Sentences queue up and play in order,
/// one `say` at a time; Stop kills the current voice and drops the queue.
/// Speeches batch two sentences per invocation: every process boundary
/// clips a little audio, so fewer, bigger speeches sound continuous.
pub struct Speaker {
    tx: Option<std::sync::mpsc::Sender<SpeakCmd>>,
    current: std::sync::Arc<std::sync::Mutex<Option<Child>>>,
}

impl Speaker {
    pub fn new(voice: Option<String>) -> Self {
        let current = std::sync::Arc::new(std::sync::Mutex::new(None::<Child>));
        let cur = current.clone();
        let (tx, rx) = std::sync::mpsc::channel::<SpeakCmd>();
        std::thread::spawn(move || {
            let mut pending: Vec<String> = Vec::new();
            let speak_now =
                |text: String, cur: &std::sync::Arc<std::sync::Mutex<Option<Child>>>| {
                    if let Some(child) = spawn_say(&text, voice.as_deref()) {
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
                            speak_now(pair, &cur);
                        }
                    }
                    SpeakCmd::Flush => {
                        if !pending.is_empty() {
                            let rest = pending.join(" ");
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
        }
    }

    pub fn say(&self, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(SpeakCmd::Say(text));
        }
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
        self.tx = None;
        if let Ok(mut guard) = self.current.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_speaks_nothing() {
        assert!(spawn_say("", None).is_none());
        assert!(spawn_say("   ", None).is_none());
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
