// myshell: the model's shell tool.
//
// This is the actual `<myshell>` — the thing the model uses to DO things
// on this machine, not just find them. The model calls it with a shell
// command and we run it in the user's real login shell (bash, zsh, ...),
// so it behaves exactly like the terminal the human sees. Output streams
// back to the model as a `tool` / `function_call_output` result.
//
// The discovery companion is myshell_explore (explore.rs): the model is
// told to hit that FIRST with a CSV when it isn't sure a tool exists,
// then run it here. Both are advertised and dispatched together.

use std::time::Duration;
use tokio::process::Command;

use crate::api::truncate;
use crate::explore;

/// The shell tool's function name. The model thinks of it as `<myshell>`.
pub const TOOL_NAME: &str = "myshell";

/// What we tell the model. Emphasise: it IS the terminal, keep commands
/// single and simple, and use myshell_explore first when unsure.
pub const TOOL_DESCRIPTION: &str = "\
Your shell on this machine. Run a shell command and you get back its \
output (and exit code). It runs in the user's real login shell (bash, \
zsh, ...), so it behaves like the terminal the human sees. Keep commands \
single, simple, and read-only unless the user asked you to change \
something. If you aren't sure a tool exists or how to use it, call \
myshell_explore FIRST with a CSV of words you think could be tools, then \
run the real command here.";

/// The JSON parameters schema advertised with the shell tool.
pub fn tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "the shell command to run, exactly as typed at a terminal"
            }
        },
        "required": ["command"]
    })
}

const SHELL_TIMEOUT_SECS: u64 = 10;
const SHELL_OUTPUT_CAP: usize = 24_000;

/// Dispatch a tool call by name to whichever local tool it names.
pub async fn execute(name: &str, arguments: &str) -> Option<String> {
    match name {
        TOOL_NAME => {
            let command = extract_command(arguments);
            Some(run_shell(&command).await)
        }
        explore::TOOL_NAME => explore::execute(name, arguments).await,
        _ => None,
    }
}

/// Pull the command out of whatever the model passed. Usually JSON
/// (`{"command":"ls"}`, but we tolerate a bare string.
fn extract_command(arguments: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(c) = v.get("command").and_then(|c| c.as_str()) {
            return c.to_string();
        }
    }
    arguments.trim().to_string()
}

/// Run the command in the user's real login shell, capturing stdout +
/// stderr, with a timeout and an output cap.
async fn run_shell(command: &str) -> String {
    if command.trim().is_empty() {
        return "myshell: no command given".to_string();
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let fut = Command::new(&shell).arg("-c").arg(command).output();
    match tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), fut).await {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let mut s = stdout;
            if !stderr.trim().is_empty() {
                s.push_str(&stderr);
            }
            if s.trim().is_empty() {
                s = format!("(no output) exit code: {}", out.status.code().unwrap_or(-1));
            } else if !out.status.success() {
                s.push_str(&format!("\nexit code: {}", out.status.code().unwrap_or(-1)));
            }
            truncate(&s, SHELL_OUTPUT_CAP)
        }
        Ok(Err(e)) => format!("myshell: could not start shell: {}", e),
        Err(_) => format!("(timed out after {}s)", SHELL_TIMEOUT_SECS),
    }
}

/// The JSON tool definitions advertised in both protocols: the shell
/// tool plus its discovery companion.
pub fn tool_defs() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "name": TOOL_NAME,
            "description": TOOL_DESCRIPTION,
            "parameters": tool_parameters(),
        }),
        serde_json::json!({
            "type": "function",
            "name": explore::TOOL_NAME,
            "description": explore::TOOL_DESCRIPTION,
            "parameters": explore::tool_parameters(),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_command_parses_json() {
        assert_eq!(extract_command("{\"command\":\"ls -la\"}"), "ls -la");
    }

    #[test]
    fn extract_command_falls_back_to_bare_string() {
        assert_eq!(extract_command("ls -la"), "ls -la");
    }

    #[tokio::test]
    async fn run_shell_echoes() {
        let out = run_shell("echo hi").await;
        assert!(out.contains("hi"));
    }

    #[tokio::test]
    async fn run_shell_reports_nonzero_exit() {
        let out = run_shell("false").await;
        assert!(out.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn run_shell_empty_command() {
        let out = run_shell("  ").await;
        assert!(out.contains("no command given"));
    }
}
