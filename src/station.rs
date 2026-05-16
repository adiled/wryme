// Stations: named bundles of (url, key, model) you can talk to. The whole
// grandma-layer of provider selection. One dial. No second decision.
//
// Sources, in order:
//   1. Built-in `demo` station — always present, talks to nobody, streams
//      canned replies. The thing a brand-new user lands on.
//   2. `WME_DEFAULT_STATION_*` env vars — defines a single "default" station.
//      If the user only sets a key, we pre-fill OpenAI defaults around it.
//   3. `~/.config/wryme/stations.toml` — any extra stations the user has
//      written down. Free-form list.
//
// Selection: `--station NAME` if given (looked up by exact name), else the
// first non-demo station in the list, else demo.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Station {
    pub name: String,
    pub url: String,
    pub key: String,
    pub model: String,
    pub is_demo: bool,
}

impl Station {
    pub fn demo() -> Self {
        Self {
            name: "demo".into(),
            url: String::new(),
            key: String::new(),
            model: "canned replies".into(),
            is_demo: true,
        }
    }
}

#[derive(Debug, Deserialize)]
struct StationsFile {
    #[serde(default)]
    station: Vec<StationDef>,
}

#[derive(Debug, Deserialize)]
struct StationDef {
    name: String,
    url: String,
    model: String,
    #[serde(default)]
    key: Option<String>,
    /// Read the key from this env var instead of inlining it. Lets users
    /// commit the config without leaking secrets.
    #[serde(default)]
    key_env: Option<String>,
}

impl StationDef {
    fn resolve(self) -> Station {
        let key = match (self.key, self.key_env) {
            (Some(k), _) => k,
            (None, Some(env_name)) => std::env::var(&env_name).unwrap_or_default(),
            (None, None) => String::new(),
        };
        Station {
            name: self.name,
            url: self.url,
            key,
            model: self.model,
            is_demo: false,
        }
    }
}

pub fn load_all() -> Result<Vec<Station>> {
    let mut out = vec![Station::demo()];

    if let Some(env_station) = from_env() {
        out.push(env_station);
    }

    if let Some(path) = config_path() {
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let parsed: StationsFile = toml::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            for def in parsed.station {
                out.push(def.resolve());
            }
        }
    }
    Ok(out)
}

fn from_env() -> Option<Station> {
    let name = std::env::var("WME_DEFAULT_STATION_NAME").ok();
    let url = std::env::var("WME_DEFAULT_STATION_URL").ok();
    let key = std::env::var("WME_DEFAULT_STATION_KEY")
        .ok()
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .unwrap_or_default();
    let model = std::env::var("WME_DEFAULT_STATION_MODEL").ok();

    // If the user has set absolutely nothing, there's no env station — we'll
    // fall through to demo.
    if name.is_none() && url.is_none() && key.is_empty() && model.is_none() {
        return None;
    }

    Some(Station {
        name: name.unwrap_or_else(|| "default".into()),
        url: url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
        key,
        model: model.unwrap_or_else(|| "gpt-4o-mini".into()),
        is_demo: false,
    })
}

fn config_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("stations.toml")
    })
}

/// Pick the active station. Honors `--station NAME` if given. Falls back to
/// the first non-demo station, then demo.
pub fn pick<'a>(stations: &'a [Station], requested: Option<&str>) -> Result<&'a Station> {
    if let Some(name) = requested {
        return stations
            .iter()
            .find(|s| s.name == name)
            .with_context(|| {
                let known: Vec<&str> = stations.iter().map(|s| s.name.as_str()).collect();
                format!(
                    "no station named '{}' — known: {}",
                    name,
                    known.join(", ")
                )
            });
    }
    Ok(stations
        .iter()
        .find(|s| !s.is_demo)
        .unwrap_or(&stations[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_is_always_there() {
        let demo = Station::demo();
        assert!(demo.is_demo);
        assert_eq!(demo.name, "demo");
    }

    #[test]
    fn pick_falls_back_to_demo_when_no_real_station() {
        let s = vec![Station::demo()];
        let p = pick(&s, None).unwrap();
        assert!(p.is_demo);
    }

    #[test]
    fn pick_prefers_first_non_demo() {
        let s = vec![
            Station::demo(),
            Station {
                name: "a".into(),
                url: "u".into(),
                key: "k".into(),
                model: "m".into(),
                is_demo: false,
            },
        ];
        assert_eq!(pick(&s, None).unwrap().name, "a");
    }

    #[test]
    fn pick_by_name() {
        let s = vec![
            Station::demo(),
            Station {
                name: "a".into(),
                url: "u".into(),
                key: "k".into(),
                model: "m".into(),
                is_demo: false,
            },
            Station {
                name: "b".into(),
                url: "u".into(),
                key: "k".into(),
                model: "m".into(),
                is_demo: false,
            },
        ];
        assert_eq!(pick(&s, Some("b")).unwrap().name, "b");
        assert!(pick(&s, Some("nope")).is_err());
    }
}
