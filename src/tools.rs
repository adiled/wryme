// The model's shell tool — named after the user's real login shell, so
// it is `zsh` on a zsh machine, `bash` on bash, etc.
//
// This is the actual shell: the thing the model uses to DO things on
// this machine, not just find them. The model calls it with a command
// and we run it in the user's real login shell (bash, zsh, ...), so it
// behaves exactly like the terminal the human sees. Output streams back
// to the model as a `tool` / `function_call_output` result.
//
// The discovery companion is `<shell>_explore` (explore.rs): the model
// is told to hit that FIRST with a CSV when it isn't sure a tool exists,
// then run it here. Long commands go async after 10s and the model can
// peek at them with `<shell>_check` (jobs.rs); a finished job's result
// is auto-delivered back into the conversation. All three are advertised
// and dispatched together.

use std::time::Duration;

use crate::explore;
use crate::jobs;

/// The shell tool's function name: the user's real login shell, e.g.
/// `zsh`, `bash`, `fish`. The model sees exactly the shell the human
/// uses.
pub fn shell_name() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    explore::shell_basename(&shell)
}

/// What we tell the model. Emphasise: it IS the terminal, keep commands
/// single and simple, and use myshell_explore first when unsure.
pub const TOOL_DESCRIPTION: &str = "\
Your shell on this machine. Run a shell command and you get back its \
output (and exit code). It runs in the user's real login shell (bash, \
zsh, ...), so it behaves like the terminal the human sees. Keep commands \
single, simple, and read-only unless the user asked you to change \
something. If you aren't sure a tool exists or how to use it, call the \
discovery tool FIRST (the one named after your shell, e.g. zsh_explore \
or bash_explore) with a CSV of words you think could be tools, then run \
the real command here.\n\nA command that takes more than 10 seconds goes asynchronous: you'll get \
'gone async · id=N'. The command keeps running in the background. Use \
the check tool (the one named after your shell plus _check, e.g. \
zsh_check) with that id to peek at its progress or get the final result; \
otherwise a finished job's result will be delivered to you on its own.";

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

/// Dispatch a tool call by name to whichever local tool it names.
pub async fn execute(name: &str, arguments: &str) -> Option<String> {
    if name == shell_name() {
        let command = extract_command(arguments);
        return Some(run_shell(&command).await);
    }
    if name == check_name() {
        let id = extract_id(arguments);
        return Some(check(id).await);
    }
    if name == explore::tool_name() {
        return explore::execute(name, arguments).await;
    }
    None
}

/// The async-job check tool: `<shell>_check`, e.g. `zsh_check`.
pub fn check_name() -> String {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    format!("{}_check", explore::shell_basename(&shell))
}

pub const CHECK_DESCRIPTION: &str = "\
Check an async shell job. Long commands that run past 10 seconds go \
asynchronous: the shell tool returns 'gone async · id=N'. Call this \
with that id to see how far it has got so far, or its final result once \
it finishes. The system may also deliver a finished job's result to you \
on its own.";

pub fn check_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "integer",
                "description": "the async job id from 'gone async · id=N'"
            }
        },
        "required": ["id"]
    })
}

/// Pull the job id out of whatever the model passed (JSON or bare).
fn extract_id(arguments: &str) -> u64 {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
            return id;
        }
    }
    arguments.trim().parse().unwrap_or(0)
}

/// Peek at (or collect) an async job's result.
async fn check(id: u64) -> String {
    match jobs::poll(id) {
        None => format!("unknown job id {id}"),
        Some(st) if st.running => {
            let so = if st.output.trim().is_empty() {
                "(no output yet)"
            } else {
                &st.output
            };
            format!("still running · so far:\n{so}")
        }
        Some(st) => {
            jobs::mark_delivered(id);
            st.output
        }
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

/// Run a command through the real login shell. Fast commands (<10s)
/// return their output straight away; slower ones become background jobs
/// and we tell the model they went async.
async fn run_shell(command: &str) -> String {
    if command.trim().is_empty() {
        return format!("{}: no command given", shell_name());
    }
    let handle = jobs::spawn(command.to_string());
    match tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), handle.done).await {
        Ok(Ok(output)) => {
            jobs::mark_delivered(handle.id);
            output
        }
        Ok(Err(_)) => format!("{}: job channel closed", shell_name()),
        Err(_) => format!("gone async · id={}", handle.id),
    }
}

/// The JSON tool definitions advertised in both protocols: the shell
/// tool, its discovery companion, and the async-job checker.
pub fn tool_defs() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "name": shell_name(),
            "description": TOOL_DESCRIPTION,
            "parameters": tool_parameters(),
        }),
        serde_json::json!({
            "type": "function",
            "name": explore::tool_name(),
            "description": explore::TOOL_DESCRIPTION,
            "parameters": explore::tool_parameters(),
        }),
        serde_json::json!({
            "type": "function",
            "name": check_name(),
            "description": CHECK_DESCRIPTION,
            "parameters": check_parameters(),
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

    #[test]
    fn extract_id_parses_json() {
        assert_eq!(extract_id("{\"id\":7}"), 7);
    }

    #[test]
    fn extract_id_falls_back_to_bare_number() {
        assert_eq!(extract_id("7"), 7);
        assert_eq!(extract_id("nope"), 0);
    }

    #[test]
    fn check_name_is_shell_suffixed() {
        let n = check_name();
        assert!(n.ends_with("_check"));
    }

    #[test]
    fn tool_defs_advertises_three_tools() {
        let defs = tool_defs();
        assert_eq!(defs.len(), 3);
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&shell_name().as_str()));
        assert!(names.contains(&explore::tool_name().as_str()));
        assert!(names.contains(&check_name().as_str()));
    }

    #[tokio::test]
    async fn execute_check_unknown_job() {
        let out = execute(&check_name(), "{\"id\":9999}").await;
        assert_eq!(out.as_deref(), Some("unknown job id 9999"));
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
