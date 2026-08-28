// Station write path. Append a new [[station]] block or update an
// existing one in-place. Read path lives in station.rs.
//
// File mutations preserve everything outside the affected block: other
// stations, comments, blank lines. Append is a simple text concat to the
// end of the file. Update finds the [[station]] block whose `name` field
// matches the target's name and replaces just those lines.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

use crate::station::{Patience, Station};

/// Append one [[station]] block to the stations file. Creates the file
/// (and parent directory) if missing. Preserves the rest of the file
/// exactly; we never rewrite anything that was already there.
pub fn append_to_file(path: &PathBuf, station: &Station) -> Result<()> {
    use std::io::Write;
    let block = serialize_block(station);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {} for append", path.display()))?;
    f.write_all(block.as_bytes())
        .with_context(|| format!("writing to {}", path.display()))?;
    Ok(())
}

/// Find the [[station]] block whose `name` matches `station.name` in the
/// file and replace it with a fresh serialization. Preserves everything
/// outside that block (other stations, comments, blank lines).
///
/// Errors if no block with that name is found, so the caller can decide
/// whether to fall back to appending or surface a message.
pub fn update_in_file(path: &PathBuf, station: &Station) -> Result<()> {
    let original = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let updated = replace_station_block(&original, station)
        .with_context(|| format!("no station '{}' in {}", station.name, path.display()))?;
    std::fs::write(path, updated)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Returns Ok(new_content) if a [[station]] block with `station.name`
/// was found and replaced. Returns Err if no such block exists.
fn replace_station_block(content: &str, station: &Station) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut replaced = false;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("[[station]]") {
            // Scan forward to find the end of this block: next [[...]]
            // header or end of file.
            let block_start = i;
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("[[") {
                i += 1;
            }
            let block_end = i; // exclusive
            let block_lines = &lines[block_start..block_end];
            if block_has_name(block_lines, &station.name) {
                // The serialized block starts with a leading newline;
                // strip it here since we are inlining, and also trim
                // trailing newlines so the surrounding spacing is owned
                // by the file, not by our serializer.
                let serialized = serialize_block(station);
                let trimmed = serialized
                    .trim_start_matches('\n')
                    .trim_end_matches('\n');
                out.push(trimmed.to_string());
                replaced = true;
            } else {
                for l in block_lines {
                    out.push((*l).to_string());
                }
            }
        } else {
            out.push(line.to_string());
            i += 1;
        }
    }
    if !replaced {
        return Err(anyhow!("station block not found"));
    }
    let mut result = out.join("\n");
    if content.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

fn block_has_name(block_lines: &[&str], name: &str) -> bool {
    let needle = format!("name = {}", toml_str(name));
    block_lines.iter().any(|l| l.trim() == needle)
}

fn serialize_block(station: &Station) -> String {
    let mut block = String::new();
    block.push('\n');
    block.push_str("[[station]]\n");
    block.push_str(&format!("name = {}\n", toml_str(&station.name)));
    block.push_str(&format!("model = {}\n", toml_str(&station.model)));
    if let Some(b) = station.dials.boldness {
        block.push_str(&format!("boldness = {}\n", b));
    }
    if let Some(p) = station.dials.patience {
        let label = match p {
            Patience::Quick => "quick",
            Patience::Steady => "steady",
            Patience::Slow => "slow",
        };
        block.push_str(&format!("patience = \"{}\"\n", label));
    }
    if let Some(v) = station.dials.verbosity {
        block.push_str(&format!("verbosity = {}\n", v));
    }
    block
}

/// TOML-quote a single-line string. Only handles backslash and
/// double-quote escapes; we only ever serialize station names and model
/// ids which are simple ascii in practice.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::Dials;

    #[test]
    fn replace_block_preserves_surrounding_content() {
        let original = "\
# header comment
[[station]]
name = \"alpha\"
model = \"m1\"

# middle comment
[[station]]
name = \"beta\"
model = \"m2\"
boldness = 0.5

[[station]]
name = \"gamma\"
model = \"m3\"
";
        let target = Station {
            name: "beta".into(),
            model: "m2-updated".into(),
            dials: Dials {
                boldness: Some(1.2),
                patience: Some(Patience::Slow),
                verbosity: None,
            },
        };
        let updated = replace_station_block(original, &target).unwrap();
        assert!(updated.contains("name = \"alpha\""));
        assert!(updated.contains("name = \"gamma\""));
        assert!(updated.contains("# header comment"));
        assert!(updated.contains("# middle comment"));
        assert!(updated.contains("model = \"m2-updated\""));
        assert!(updated.contains("boldness = 1.2"));
        assert!(updated.contains("patience = \"slow\""));
        assert!(!updated.contains("model = \"m2\"\n"));
        assert!(!updated.contains("boldness = 0.5"));
    }

    #[test]
    fn replace_block_errors_when_not_found() {
        let original = "\
[[station]]
name = \"alpha\"
model = \"m1\"
";
        let target = Station {
            name: "nonexistent".into(),
            model: "m".into(),
            dials: Dials::default(),
        };
        assert!(replace_station_block(original, &target).is_err());
    }
}
