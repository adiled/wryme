// shell_env — the shell tool's environment, made deterministic.
//
// The whole point of wryme is that its shell tool IS the machine: no other
// tools, no MCP. So the environment that shell runs in — and the world the
// discovery tool (<shell>_explore) reports — must be the same no matter who
// launched wme. A GUI-launched app (Dock, Finder, a .desktop file) inherits
// launchd's minimal PATH (/usr/bin:/bin:/usr/sbin:/sbin) and no $SHELL at
// all, so without this, desktop wme would discover and run commands in a
// stripped-down world that isn't the user's machine.
//
// bootstrap() fixes it from first principles, once, before anything else:
// resolve the user's real login shell, ask it in login mode (`-l`) what its
// PATH is, then adopt both into the process env. From then on every path
// that reads env (explore discovery, jobs, voice) sees exactly the login
// environment a terminal would — for any launcher.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

static LOGIN_SHELL: OnceLock<String> = OnceLock::new();
static LOGIN_PATH: OnceLock<Option<String>> = OnceLock::new();

/// The user's real login shell, resolved once and cached.
///
/// Order: `$SHELL` if it names a real binary (covers a wme launched from a
/// terminal, where the shell set it), else the account shell from the
/// directory service (macOS `dscl`; covers GUI-launched wme, where launchd
/// sets no `SHELL`), else `/bin/sh`. Never returns empty.
pub fn shell() -> &'static str {
    LOGIN_SHELL.get_or_init(resolve_login_shell)
}

/// The PATH a login shell computes, resolved once by actually asking the
/// login shell (`shell -l -c 'printf %s "$PATH"'`) and cached. Falls back
/// to the current process PATH if asking fails or hangs.
pub fn path() -> String {
    LOGIN_PATH
        .get_or_init(probe_login_path)
        .clone()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

/// Call once, first thing in main. Adopts the login environment into the
/// process env so every subsystem sees the same world a terminal would.
pub fn bootstrap() {
    let shell = shell().to_string();
    let login_path = path();
    // SAFETY: single-threaded at startup, before the tokio runtime spins
    // up; nothing reads these variables concurrently here.
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

/// Pull `UserShell: /bin/zsh` (or the multi-line `UserShell:` + indented
/// value form) out of `dscl -read` output.
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
            // `UserShell:` alone: the value sits indented on the next line.
            return lines.get(i + 1).and_then(|n| {
                let n = n.trim();
                (n.starts_with('/') && Path::new(n).is_file()).then(|| n.to_string())
            });
        }
    }
    None
}

/// Ask the login shell what `$PATH` is. The shell prepends its rc-derived
/// dirs onto whatever it inherited, so for a terminal-launched wme this is
/// the full interactive PATH, and for a GUI-launched wme it is exactly the
/// PATH the shell tool will execute with (`jobs.rs` runs `shell -l -c`).
/// Capped at 5s in case a slow rc hangs startup.
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    // Output is a single short line; the pipe can't fill up here.
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
