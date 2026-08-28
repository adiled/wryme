// myshell_explore: a discovery tool for the model.
//
// When wme hands a model a shell (the `<myshell>` idea), the model
// doesn't know what's on this machine, and it's too stupid to go look —
// it just says "i can't find that tool". This tool fixes that.
//
// The model is told to hit it FIRST with a CSV of words/phrases it
// thinks could be tools. We deterministically find each one on the
// system — PATH binaries, aliases, functions in the user's shell rc
// files — and return its --help output, so the model knows exactly what
// exists and how to use it. No LLM guessing; pure filesystem + `--help`.
//
// Both the Responses and Chat Completions protocols advertise and run
// this same tool; the per-protocol code lives in api_responses.rs and
// api_chat.rs. This file is just the tool itself: what it is called,
// how it is advertised, and what it does.

use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::api::truncate;

/// The tool name the model calls.
pub const TOOL_NAME: &str = "myshell_explore";

/// What we tell the model about the tool. The point is to make it hit
/// this FIRST with a CSV of words it thinks could be tools.
pub const TOOL_DESCRIPTION: &str = "\
Call this FIRST whenever you need to do something on this machine but \
aren't sure a command exists or how to use it. Pass a CSV of the words \
or short phrases you think could be tools. For each one we find it on \
the system (PATH binaries, shell aliases, shell functions in the user's \
rc files) and return its --help output, so you know exactly what is \
available and how to use it. Once you know what to run, execute it with \
your shell tool (myshell).";

/// The JSON parameters schema advertised with the tool.
pub fn tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "csv": {
                "type": "string",
                "description": "comma-separated words or short phrases you think could be tools"
            }
        },
        "required": ["csv"]
    })
}

/// Execute a tool call. Returns the output text to feed back to the
/// model, or None if the tool isn't one we run locally. Async because
/// gathering `--help` shells out and we want it cancellable/timeoutable.
pub async fn execute(name: &str, arguments: &str) -> Option<String> {
    if name != TOOL_NAME {
        return None;
    }
    let csv = extract_csv(arguments);
    Some(explore(&csv).await)
}

/// Pull the CSV out of whatever the model passed. Usually it's JSON
/// (`{"csv":"ls, git"}`), but we tolerate a bare string or an array.
fn extract_csv(arguments: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) {
        if let Some(c) = v.get("csv").and_then(|c| c.as_str()) {
            return c.to_string();
        }
        if let Some(c) = v.get("csv").and_then(|c| c.as_array()) {
            return c
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(",");
        }
    }
    arguments.trim().to_string()
}

/// Find each term on the machine and assemble a plain-text report.
pub async fn explore(csv: &str) -> String {
    let terms = split_csv(csv);
    if terms.is_empty() {
        return "myshell_explore: no terms given".to_string();
    }
    let bins = path_bins();
    let rc = rc_entries();

    let mut out = String::new();
    for term in &terms {
        out.push_str(&format!("== {} ==\n", term));
        if let Some(path) = which(term) {
            out.push_str(&format!("path: {}\n", path));
            if let Some(h) = binary_help(&path).await {
                out.push_str(&format!("help:\n{}\n", h));
            }
        } else if let Some(e) = rc_exact(term, &rc) {
            out.push_str(&format!("{} in {}: {}\n", e.kind, e.file, e.def));
        } else {
            let fbins = fuzzy(term, &bins);
            let frc = fuzzy_rc(term, &rc);
            if fbins.is_empty() && frc.is_empty() {
                out.push_str("not found on PATH or rc files\n");
            } else {
                for name in fbins {
                    if let Some(path) = which(&name) {
                        out.push_str(&format!("fuzzy: {} -> {}\n", name, path));
                        if let Some(h) = binary_help(&path).await {
                            out.push_str(&format!("help:\n{}\n", h));
                        }
                    }
                }
                for e in frc {
                    out.push_str(&format!(
                        "fuzzy: {} ({} in {}): {}\n",
                        e.name, e.kind, e.file, e.def
                    ));
                }
            }
        }
    }
    truncate(&out, 24_000)
}

// ---- csv ----

fn split_csv(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ---- discovery ----

/// Every executable-looking filename on PATH, deduped and sorted.
fn path_bins() -> Vec<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in path.split(':') {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if name.starts_with('.') {
                        continue;
                    }
                    if seen.insert(name.clone()) {
                        out.push(name);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Resolve an exact term to a real binary path on PATH.
fn which(term: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let p = Path::new(dir).join(term);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// One alias or function pulled out of a shell rc file.
#[derive(Clone, Debug)]
struct RcEntry {
    name: String,
    kind: String,
    def: String,
    file: String,
}

fn rc_exact(term: &str, rc: &[RcEntry]) -> Option<RcEntry> {
    rc.iter().find(|e| e.name == term).cloned()
}

fn fuzzy_rc(term: &str, rc: &[RcEntry]) -> Vec<RcEntry> {
    let tl = term.to_lowercase();
    rc.iter()
        .filter(|e| {
            let nl = e.name.to_lowercase();
            nl.starts_with(&tl) || nl.contains(&tl)
        })
        .take(5)
        .cloned()
        .collect()
}

/// Case-insensitive prefix/contains match against a list of names.
fn fuzzy(term: &str, names: &[String]) -> Vec<String> {
    let tl = term.to_lowercase();
    names
        .iter()
        .filter(|n| {
            let nl = n.to_lowercase();
            nl.starts_with(&tl) || nl.contains(&tl)
        })
        .take(5)
        .cloned()
        .collect()
}

/// Pull aliases and functions out of the user's shell rc files.
fn rc_entries() -> Vec<RcEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    for f in ["~/.zshrc", "~/.bashrc", "~/.profile", "~/.config/fish/config.fish"] {
        let rel = f.trim_start_matches('~').trim_start_matches('/');
        let path = Path::new(&home).join(rel);
        if !path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("alias ") {
                    let rest = rest.trim();
                    let name = rest.split('=').next().unwrap_or("").trim();
                    if !name.is_empty() {
                        out.push(RcEntry {
                            name: name.into(),
                            kind: "alias".into(),
                            def: t.to_string(),
                            file: f.to_string(),
                        });
                    }
                } else if let Some(name) = function_name(t) {
                    out.push(RcEntry {
                        name,
                        kind: "function".into(),
                        def: t.to_string(),
                        file: f.to_string(),
                    });
                }
            }
        }
    }
    out
}

/// Name of a shell function declared on this one line, if any.
/// Handles `foo() {`, `foo () {`, and `function foo {`.
fn function_name(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("function ") {
        return rest.split_whitespace().next().map(String::from);
    }
    if t.contains('(') && t.contains('{') {
        let name = t.split('(').next()?.trim();
        if !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Some(name.to_string());
        }
    }
    None
}

// ---- help ----

/// Run the binary's own help: `--help`, then `-h`, then `man` as a
/// fallback. Returns the text, or None if nothing useful came back.
async fn binary_help(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_string_lossy().into_owned();

    for args in [["--help"], ["-h"]] {
        if let Some(o) = run(path, &args).await {
            let o = o.trim();
            if !o.is_empty() {
                return Some(truncate(o, 4000));
            }
        }
    }
    if let Some(o) = run("man", &[&name]).await {
        let o = o.trim();
        if !o.is_empty() {
            return Some(truncate(&first_lines(o, 30), 4000));
        }
    }
    None
}

/// Run a command with a timeout. Captures stdout, or stderr if stdout
/// is empty. None on timeout or launch failure.
async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let fut = Command::new(cmd).args(args).output();
    match tokio::time::timeout(Duration::from_secs(3), fut).await {
        Ok(Ok(out)) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if s.trim().is_empty() {
                s = String::from_utf8_lossy(&out.stderr).into_owned();
            }
            Some(s)
        }
        _ => None,
    }
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_csv_handles_quotes_and_spaces() {
        let terms = split_csv(" ls, git , \"git rebase\", , 'brew' ");
        assert_eq!(terms, vec!["ls", "git", "git rebase", "brew"]);
    }

    #[test]
    fn extract_csv_parses_json() {
        assert_eq!(extract_csv("{\"csv\":\"ls, git\"}"), "ls, git");
        assert_eq!(extract_csv("{\"csv\":[\"ls\",\"git\"]}"), "ls,git");
    }

    #[test]
    fn extract_csv_falls_back_to_bare_string() {
        assert_eq!(extract_csv("ls,git"), "ls,git");
    }

    #[test]
    fn fuzzy_matches_prefix_and_contains() {
        let names: Vec<String> = vec!["git".into(), "git-lfs".into(), "grep".into(), "rg".into()];
        let got = fuzzy("git", &names);
        assert_eq!(got, vec!["git", "git-lfs"]);
        let got2 = fuzzy("rg", &names);
        assert_eq!(got2, vec!["rg"]);
    }

    #[test]
    fn function_name_parses_declarations() {
        assert_eq!(function_name("foo() {"), Some("foo".into()));
        assert_eq!(function_name("foo () {"), Some("foo".into()));
        assert_eq!(function_name("function foo {"), Some("foo".into()));
        assert_eq!(function_name("alias foo='bar'"), None);
    }

    #[test]
    fn which_and_bins_find_a_path_binary() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("wryme_explore_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let bin = dir.join("hello_tool");
        fs::write(&bin, "#!/bin/sh\necho hello\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

        let old = std::env::var("PATH").ok();
        std::env::set_var("PATH", &dir);
        assert_eq!(which("hello_tool").unwrap(), bin.display().to_string());
        assert!(path_bins().contains(&"hello_tool".to_string()));
        if let Some(p) = old {
            std::env::set_var("PATH", p);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}