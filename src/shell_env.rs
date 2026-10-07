use std::path::Path;
use std::sync::OnceLock;
#[cfg(not(target_arch = "wasm32"))]
use std::process::Command;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

static LOGIN_SHELL: OnceLock<String> = OnceLock::new();
static LOGIN_PATH: OnceLock<Option<String>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(target_arch = "wasm32"))]
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn shell() -> &'static str {
    LOGIN_SHELL.get_or_init(resolve_login_shell)
}

pub fn path() -> String {
    LOGIN_PATH
        .get_or_init(probe_login_path)
        .clone()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

pub fn bootstrap() {
    let shell = shell().to_string();
    let login_path = path();
    unsafe {
        std::env::set_var("SHELL", &shell);
        std::env::set_var("PATH", &login_path);
    }
    tracing::debug!(shell = %shell, path = %login_path, "adopted login shell environment");
}

fn resolve_login_shell() -> String {
    if let Ok(s) = std::env::var("SHELL") {
        let s = s.trim().to_string();
        if !s.is_empty() && Path::new(&s).is_file() {
            return s;
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(u) = std::env::var("USER") {
        let u = u.trim();
        if !u.is_empty()
            && let Ok(out) = Command::new("/usr/bin/dscl")
                .args([".", "-read", &format!("/Users/{u}"), "UserShell"])
                .output()
            && out.status.success()
            && let Some(s) = user_shell_from_dscl(&out.stdout)
        {
            return s;
        }
    }
    "/bin/sh".to_string()
}

#[cfg(target_os = "macos")]
fn user_shell_from_dscl(out: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(out);
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.trim().strip_prefix("UserShell:") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return (Path::new(rest).is_file()).then(|| rest.to_string());
            }
            return lines.get(i + 1).and_then(|n| {
                let n = n.trim();
                (n.starts_with('/') && Path::new(n).is_file()).then(|| n.to_string())
            });
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn probe_login_path() -> Option<String> {
    let shell = shell();
    let mut child = Command::new(shell)
        .arg("-l")
        .arg("-c")
        .arg(r#"printf %s "$PATH""#)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8(out.stdout).ok()?;
    let p = p.trim();
    if p.is_empty() {
        None
    } else {
        Some(p.to_string())
    }
}

/// No shell to probe from the browser.
#[cfg(target_arch = "wasm32")]
fn probe_login_path() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_resolves_to_a_real_file() {
        let s = shell();
        assert!(s.starts_with('/'));
        assert!(Path::new(s).is_file());
    }

    #[test]
    fn login_path_contains_system_bins() {
        let p = path();
        assert!(p.split(':').any(|d| d == "/usr/bin"));
    }
}
