use anyhow::{Context, Result, anyhow};

use crate::station::Station;

pub fn save_new(station: &Station) -> Result<()> {
    let mut text = crate::config::stations_text().unwrap_or_default();
    text.push_str(&serialize_block(station));
    crate::config::write_stations(&text)
}

pub fn update(station: &Station) -> Result<()> {
    let original = crate::config::stations_text().context("no stations.toml")?;
    let updated = replace_station_block(&original, station)?;
    crate::config::write_stations(&updated)
}

fn replace_station_block(content: &str, station: &Station) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut replaced = false;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("[[station]]") {
            let block_start = i;
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("[[") {
                i += 1;
            }
            let block_end = i;
            let block_lines = &lines[block_start..block_end];
            if block_has_name(block_lines, &station.name) {
                out.push(serialize_block_inline(station));
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
        block.push_str(&format!("patience = \"{}\"\n", p.label()));
    }
    if let Some(b) = station.dials.brainy {
        block.push_str(&format!("brainy = \"{}\"\n", b.label()));
    }
    if station.dials.tinker_keep != crate::station::TinkerKeep::All {
        block.push_str(&format!(
            "tinker_keep = \"{}\"\n",
            station.dials.tinker_keep.label()
        ));
    }
    if station.dials.tinker_clip != crate::station::TinkerVal::All {
        block.push_str(&format!(
            "tinker_clip = \"{}\"\n",
            station.dials.tinker_clip.label()
        ));
    }
    if station.dials.tinker_depth != crate::station::TinkerVal::All {
        block.push_str(&format!(
            "tinker_depth = \"{}\"\n",
            station.dials.tinker_depth.label()
        ));
    }
    if let Some(voice) = &station.voice {
        block.push_str(&format!("voice = {}\n", toml_str(voice)));
    }
    block
}

fn serialize_block_inline(station: &Station) -> String {
    serialize_block(station)
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .to_string()
}

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
    use crate::station::{Dials, Patience};

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
                brainy: Some(crate::station::Brainy::Murmur),
                tinker_keep: crate::station::TinkerKeep::All,
                tinker_clip: crate::station::TinkerVal::All,
                tinker_depth: crate::station::TinkerVal::All,
            },
            voice: None,
        };
        let updated = replace_station_block(original, &target).unwrap();
        assert!(updated.contains("name = \"alpha\""));
        assert!(updated.contains("name = \"gamma\""));
        assert!(updated.contains("# header comment"));
        assert!(updated.contains("# middle comment"));
        assert!(updated.contains("model = \"m2-updated\""));
        assert!(updated.contains("boldness = 1.2"));
        assert!(updated.contains("patience = \"slow\""));
        assert!(updated.contains("brainy = \"murmur\""));
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
            voice: None,
        };
        assert!(replace_station_block(original, &target).is_err());
    }
}
