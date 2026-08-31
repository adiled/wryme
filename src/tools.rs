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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::book::{self, Bookmark};
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

/// Dispatch a tool call by name to whichever local tool it names. `engine`
/// is the shared book engine (used by the invisible `book` tool) and
/// `session` is this turn's assembled messages (used by `append`).
pub async fn execute(
    engine: &Arc<Mutex<book::Engine>>,
    name: &str,
    arguments: &str,
) -> Option<String> {
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
    if name == book_name() {
        return Some(book_execute(engine, arguments).await);
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

/// The invisible bookkeeping tool: how the same agent that talks also
/// remembers. Grandma never sees it — the UI treats it as a quiet
/// "reminiscing…" instead of a tool. The model calls it in the flow to
/// look up the book, promote a compartment to the preamble, or file the
/// current thread away with its own distilled bookmark.
pub const BOOK_NAME: &str = "book";

pub fn book_name() -> &'static str {
    BOOK_NAME
}

pub const BOOK_DESCRIPTION: &str = "\
Quiet memory — this is how you remember across windows and restarts. \
Call it in the flow, without announcing it. A memory is a page: a \
distilled bookmark plus the stretches of conversation it points into. \
Pages are addressed by their topic — never a number. Actions:\n\
  find    {query} — search memory for a page. Returns matched pages.\n  open    {topic} — pull a page into this conversation as its \
          preamble; you get its distilled state and the whole thread.\n  read    {topic} — read a page's thread without promoting it.\n  deem    {topic, tags, people, facts, plans, open} — attribute this \
          stretch of conversation to the page named by topic and refresh \
          its distilled bookmark (what you remember, so the next visit \
          starts where we left off). If no page with that topic exists, \
          this stretch BIRTHS it — there is no separate \"create\" step. \
          The engine writes every turn continuously; deem points the new \
          rows at a page. A thread can be deemed into several pages over \
          its lifetime. If a second page is needed, name it distinctly \
          (e.g. \"roses\", not \"garden\" again).\n  dismiss {topic} — stop carrying a page's preamble.\n\nUse find when someone says \"remember when…\" or the early words could \
match an old page; open what matches. When a thread wraps or drifts, \
deem it so it is never lost. Keep the distilled bookmark short — \
people, facts, plans, and what is still open.";

pub fn book_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["find", "open", "read", "deem", "dismiss"],
                "description": "what to do"
            },
            "query": {
                "type": "string",
                "description": "words to search memory for (for find)"
            },
            "topic": {
                "type": "string",
                "description": "the page's name — the only handle you use (for open/read/deem/dismiss)"
            },
            "tags": {
                "type": "array", "items": { "type": "string" },
                "description": "searchable tags"
            },
            "people": {
                "type": "array", "items": { "type": "string" }
            },
            "facts": {
                "type": "array", "items": { "type": "string" }
            },
            "plans": {
                "type": "array", "items": { "type": "string" }
            },
            "open": {
                "type": "array", "items": { "type": "string" },
                "description": "open threads — where we left off"
            }
        },
        "required": ["action"]
    })
}

/// True for the tools grandma never sees: the bookkeeper and the phantom
/// async-job checker. The UI suppresses the "tinkering…" label and the
/// tool name for these.
pub fn is_hidden_tool(name: &str) -> bool {
    name == BOOK_NAME || name == check_name()
}

/// True for the bookkeeping tool specifically — the UI shows it as a
/// quiet "reminiscing…" instead of a tool.
pub fn is_book_tool(name: &str) -> bool {
    name == BOOK_NAME
}

/// Run the invisible book tool. Locks the shared engine. The engine
/// records every turn itself, so `deem` just points the unattributed
/// rows at a compartment.
async fn book_execute(
    engine: &Arc<Mutex<book::Engine>>,
    arguments: &str,
) -> String {
    let v: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return format!("{BOOK_NAME}: could not parse arguments"),
    };
    let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
    match action {
        "find" => {
            let query = v.get("query").and_then(|q| q.as_str()).unwrap_or("").to_string();
            let mut e = engine.lock().unwrap();
            e.note_lookup();
            let hits = book::match_compartments(&e.book, &query);
            render_find(&query, &hits)
        }
        "open" => {
            let topic = v.get("topic").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let mut e = engine.lock().unwrap();
            match e.open(&topic) {
                Ok(Some(text)) => text,
                Ok(None) => format!("no page \"{topic}\""),
                Err(err) => format!("{BOOK_NAME}: {err:#}"),
            }
        }
        "read" => {
            let topic = v.get("topic").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let e = engine.lock().unwrap();
            match book::read_compartment(&e.book, &topic) {
                Ok(Some(msgs)) if !msgs.is_empty() => book::render_compartment(&msgs),
                Ok(_) => format!("page \"{topic}\" has no thread yet"),
                Err(err) => format!("{BOOK_NAME}: {err:#}"),
            }
        }
        "deem" => {
            let bookmark = bookmark_from(&v);
            let mut e = engine.lock().unwrap();
            match e.deem(&bookmark) {
                Ok(out) => out,
                Err(err) => format!("{BOOK_NAME}: {err:#}"),
            }
        }
        "dismiss" => {
            let topic = v.get("topic").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let mut e = engine.lock().unwrap();
            e.dismiss(&topic);
            format!("dropped \"{topic}\" from the preamble")
        }
        _ => format!("{BOOK_NAME}: unknown action '{action}'"),
    }
}

fn render_find(query: &str, hits: &[&book::CompartmentMeta]) -> String {
    if hits.is_empty() {
        return format!("no compartments match \"{query}\"");
    }
    let mut out = format!("pages matching \"{query}\":\n");
    for m in hits {
        out.push_str(&format!(
            "  {} — open: {}\n",
            m.topic,
            m.open.join(", ")
        ));
    }
    out
}

fn bookmark_from(v: &serde_json::Value) -> Bookmark {
    Bookmark {
        topic: v.get("topic").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        tags: str_list(v, "tags"),
        people: str_list(v, "people"),
        facts: str_list(v, "facts"),
        plans: str_list(v, "plans"),
        open: str_list(v, "open"),
    }
}

fn str_list(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The JSON tool definitions advertised in both protocols: the shell
/// tool, its discovery companion, the async-job checker, and the
/// invisible bookkeeper.
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
        serde_json::json!({ "type": "function", "name": check_name(), "description": CHECK_DESCRIPTION, "parameters": check_parameters() }),
        serde_json::json!({
            "type": "function",
            "name": book_name(),
            "description": BOOK_DESCRIPTION,
            "parameters": book_parameters(),
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
    fn tool_defs_advertises_four_tools() {
        let defs = tool_defs();
        assert_eq!(defs.len(), 4);
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&shell_name().as_str()));
        assert!(names.contains(&explore::tool_name().as_str()));
        assert!(names.contains(&check_name().as_str()));
        assert!(names.contains(&book_name()));
    }

    #[tokio::test]
    async fn book_tool_deem_births_find_open_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wryme_btool_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Arc::new(Mutex::new(book::open_engine(&dir).unwrap()));

        {
            let mut e = engine.lock().unwrap();
            e.record_turn("user", "let's plan lisbon");
            e.record_turn("assistant", "may is nice");
        }

        // No separate create step — deeming into an unborn topic births it.
        let out = execute(
            &engine,
            book_name(),
            r#"{"action":"deem","topic":"Lisbon trip","open":["comparing prices"]}"#,
        )
        .await
        .unwrap();
        assert!(out.contains("deemed rows 0..2"));

        let out = execute(
            &engine,
            book_name(),
            "{\"action\":\"find\",\"query\":\"lisbon\"}",
        )
        .await
        .unwrap();
        assert!(out.contains("Lisbon trip"));

        let out = execute(
            &engine,
            book_name(),
            "{\"action\":\"open\",\"topic\":\"Lisbon trip\"}",
        )
        .await
        .unwrap();
        assert!(out.contains("let's plan lisbon"));
        assert!(out.contains("comparing prices"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn book_tool_deem_advances_watermark() {
        let dir = std::env::temp_dir().join(format!("wryme_btool2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Arc::new(Mutex::new(book::open_engine(&dir).unwrap()));

        {
            let mut e = engine.lock().unwrap();
            e.record_turn("user", "hello");
        }
        let out = execute(
            &engine,
            book_name(),
            "{\"action\":\"deem\",\"topic\":\"t\"}",
        )
        .await
        .unwrap();
        assert!(out.contains("deemed rows 0..1"));

        {
            let mut e = engine.lock().unwrap();
            e.record_turn("user", "hi");
        }
        let out2 = execute(
            &engine,
            book_name(),
            "{\"action\":\"deem\",\"topic\":\"t\"}",
        )
        .await
        .unwrap();
        assert!(out2.contains("deemed rows 1..2"));

        // Nothing new to deem now.
        let out3 = execute(
            &engine,
            book_name(),
            "{\"action\":\"deem\",\"topic\":\"t\"}",
        )
        .await
        .unwrap();
        assert!(out3.contains("no new turns"));
        let _ = std::fs::remove_dir_all(&dir);
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
