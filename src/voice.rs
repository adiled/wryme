use std::process::{Child, Command};

pub const DEFAULT_MAC_VOICE: &str = "Samantha";

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

pub fn speak(text: &str, voice: Option<&str>) -> Option<Child> {
    let body: String = text.chars().take(3000).collect();
    if body.trim().is_empty() {
        return None;
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("say");
        c.arg("-v").arg(voice.unwrap_or(DEFAULT_MAC_VOICE));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_speaks_nothing() {
        assert!(speak("", None).is_none());
        assert!(speak("   ", None).is_none());
    }

    #[test]
    fn backend_detection_finds_path_bins() {
        assert!(has_bin("sh"));
        assert!(!has_bin("definitely-not-a-real-binary-xyz"));
    }
}
