#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use tokio::process::Command;

use crate::api::truncate;

const REPORT_MAX_CHARS: usize = 24_000;
const HELP_MAX_CHARS: usize = 4_000;
const MAN_MAX_LINES: usize = 30;
const FUZZY_MAX_RESULTS: usize = 5;
#[cfg(not(target_arch = "wasm32"))]
const HELP_TIMEOUT: Duration = Duration::from_secs(3);
const RC_FILES: &[&str] = &[
    "~/.zshrc",
    "~/.bashrc",
    "~/.profile",
    "~/.config/fish/config.fish",
];

pub fn tool_name() -> String {
    let shell = crate::shell_env::shell();
    format!("{}_explore", shell_basename(shell))
}

pub(crate) fn shell_basename(shell: &str) -> String {
    Path::new(shell)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sh".to_string())
}

pub const TOOL_DESCRIPTION: &str = "\
Call this FIRST whenever you need to do something on this machine but \
aren't sure a command exists or how to use it. Pass a CSV of the words \
or short phrases you think could be tools. For each one we find it on \
the system (PATH binaries, shell aliases, shell functions in the user's \
rc files) and return its --help output, so you know exactly what is \
available and how to use it. Once you know what to run, execute it with \
your shell tool (the one named after your shell, e.g. zsh or bash).";

pub fn tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "csv": {
                "type": "string",
                "description": "comma-separated words or short phrases you think could be tools"
            }
        },
        "required": ["csv"],
        "additionalProperties": false
    })
}

pub async fn execute(name: &str, arguments: &str) -> Option<String> {
    if name != tool_name() {
        return None;
    }
    let csv = extract_csv(arguments);
    Some(explore(&csv).await)
}

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

pub async fn explore(csv: &str) -> String {
    let terms = split_csv(csv);
    if terms.is_empty() {
        return format!(
            "{}: no terms given — call again with {{\"csv\": \"word1, word2\"}}",
            tool_name()
        );
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
    truncate(&out, REPORT_MAX_CHARS)
}

fn split_csv(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

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
        .take(FUZZY_MAX_RESULTS)
        .cloned()
        .collect()
}

fn fuzzy(term: &str, names: &[String]) -> Vec<String> {
    let tl = term.to_lowercase();
    names
        .iter()
        .filter(|n| {
            let nl = n.to_lowercase();
            nl.starts_with(&tl) || nl.contains(&tl)
        })
        .take(FUZZY_MAX_RESULTS)
        .cloned()
        .collect()
}

fn rc_entries() -> Vec<RcEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    for f in RC_FILES {
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

fn function_name(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("function ") {
        return rest.split_whitespace().next().map(String::from);
    }
    if t.contains('(') && t.contains('{') {
        let name = t.split('(').next()?.trim();
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Some(name.to_string());
        }
    }
    None
}

async fn binary_help(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_string_lossy().into_owned();

    for args in [["--help"], ["-h"]] {
        if let Some(o) = run(path, &args).await {
            let o = o.trim();
            if !o.is_empty() {
                return Some(truncate(o, HELP_MAX_CHARS));
            }
        }
    }
    if let Some(o) = run("man", &[&name]).await {
        let o = o.trim();
        if !o.is_empty() {
            return Some(truncate(&first_lines(o, MAN_MAX_LINES), HELP_MAX_CHARS));
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let fut = Command::new(cmd).args(args).output();
    match tokio::time::timeout(HELP_TIMEOUT, fut).await {
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

/// No subprocesses in the browser — the shop runs explore.
#[cfg(target_arch = "wasm32")]
async fn run(_cmd: &str, _args: &[&str]) -> Option<String> {
    None
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
    fn shell_basename_from_full_path() {
        assert_eq!(shell_basename("/bin/zsh"), "zsh");
        assert_eq!(shell_basename("/usr/bin/bash"), "bash");
        assert_eq!(shell_basename("/bin/sh"), "sh");
        assert_eq!(shell_basename(""), "sh");
    }

    #[test]
    fn tool_name_ends_with_explore() {
        assert!(tool_name().ends_with("_explore"));
        assert!(!tool_name().contains(' '));
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
        unsafe { std::env::set_var("PATH", &dir) };
        assert_eq!(which("hello_tool").unwrap(), bin.display().to_string());
        assert!(path_bins().contains(&"hello_tool".to_string()));
        if let Some(p) = old {
            unsafe { std::env::set_var("PATH", p) };
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
