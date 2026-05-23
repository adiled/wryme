// Stations: the recipe / preset side of the equation. A station says
// "I am using THIS model with THESE dials." It does not know or care
// which shop will actually run it; resolution happens at startup by
// matching the station's model name against shops' advertised models.
//
// Three dials, all optional. Unset means the model uses whatever default
// its maker chose. Set means the user has opinions.
//
//   - boldness   (temperature)       how loose / creative / unpredictable
//   - patience   (reasoning effort)  how hard the model deliberates
//   - verbosity  (max output tokens) the most the model is allowed to say
//
// Stations explicitly do NOT carry system prompts, tools, permissions,
// or anything else that constitutes "an agent." Those will live in a
// future preset/persona concept that wraps a station and adds extras.
// Station stays a pure dials-and-model thing.
//
// Sources, in order:
//   1. Built-in demo station. Always present.
//   2. WME_DEFAULT_STATION_MODEL env var. Optional model pin for env-only users.
//   3. ~/.config/wryme/stations.toml. Named, saved stations.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

use crate::shop::Shop;

#[derive(Debug, Clone)]
pub struct Station {
    pub name: String,
    pub model: String,
    pub dials: Dials,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Dials {
    /// Temperature. 0.0 to 2.0, conventionally. Unset = let the model
    /// pick its own default.
    pub boldness: Option<f32>,
    /// Reasoning effort. quick / steady / slow. Only meaningful on models
    /// that support extended thinking. Translated to "low"/"medium"/"high"
    /// on the wire for the Responses protocol; silently dropped for Chat
    /// Completions since it has no equivalent.
    pub patience: Option<Patience>,
    /// Max output tokens. Hard ceiling on reply length. Unset = let the
    /// model stop when it thinks it is done.
    pub verbosity: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patience {
    Quick,
    Steady,
    Slow,
}

impl Patience {
    pub fn as_wire(self) -> &'static str {
        match self {
            Patience::Quick => "low",
            Patience::Steady => "medium",
            Patience::Slow => "high",
        }
    }
}

impl Station {
    pub fn demo() -> Self {
        Self {
            name: "demo".into(),
            model: "canned replies".into(),
            dials: Dials::default(),
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
    model: String,
    #[serde(default)]
    boldness: Option<f32>,
    #[serde(default)]
    patience: Option<PatienceField>,
    #[serde(default)]
    verbosity: Option<u32>,
}

/// Accept either an enum string ("quick"/"steady"/"slow") or, for the
/// people who liked the wire format, "low"/"medium"/"high". Anything else
/// is treated as unset.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PatienceField {
    Named(String),
}

impl PatienceField {
    fn into_patience(self) -> Option<Patience> {
        let PatienceField::Named(s) = self;
        match s.to_lowercase().as_str() {
            "quick" | "low" => Some(Patience::Quick),
            "steady" | "medium" => Some(Patience::Steady),
            "slow" | "high" => Some(Patience::Slow),
            _ => None,
        }
    }
}

impl StationDef {
    fn resolve(self) -> Station {
        Station {
            name: self.name,
            model: self.model,
            dials: Dials {
                boldness: self.boldness,
                patience: self.patience.and_then(|p| p.into_patience()),
                verbosity: self.verbosity,
            },
        }
    }
}

pub fn load_all() -> Result<Vec<Station>> {
    let mut out = vec![Station::demo()];

    if let Some(env_st) = from_env() {
        out.push(env_st);
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
    let model = std::env::var("WME_DEFAULT_STATION_MODEL").ok()?;
    let name = std::env::var("WME_DEFAULT_STATION_NAME").unwrap_or_else(|_| "default".into());
    Some(Station {
        name,
        model,
        dials: Dials::default(),
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

/// Pick the active station given the loaded list, the loaded shops, and
/// an optional explicit name. Falls back to synthesizing a default from
/// the first shop's first model when nothing else matches.
pub fn pick(stations: &[Station], shops: &[Shop], requested: Option<&str>) -> Result<Station> {
    if let Some(name) = requested {
        return stations
            .iter()
            .find(|s| s.name == name)
            .cloned()
            .with_context(|| {
                let known: Vec<&str> = stations.iter().map(|s| s.name.as_str()).collect();
                format!("no station named '{}'. known: {}", name, known.join(", "))
            });
    }
    // Prefer the first non-demo station the user has saved.
    if let Some(st) = stations.iter().find(|s| s.name != "demo") {
        return Ok(st.clone());
    }
    // No saved stations. Synthesize one from the first non-demo shop's
    // first advertised model. Convention says that is the newest.
    if let Some(shop) = shops.iter().find(|s| s.name != "demo") {
        if let Some(model) = shop.models.first() {
            return Ok(Station {
                name: "untitled".into(),
                model: model.clone(),
                dials: Dials::default(),
            });
        }
    }
    // Nothing configured at all. Demo.
    Ok(Station::demo())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shop::{Protocol, Shop};

    fn shop(name: &str, models: &[&str]) -> Shop {
        Shop {
            name: name.into(),
            url: "u".into(),
            key: "".into(),
            protocol: Protocol::ChatCompletions,
            models: models.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn station(name: &str, model: &str) -> Station {
        Station {
            name: name.into(),
            model: model.into(),
            dials: Dials::default(),
        }
    }

    #[test]
    fn demo_station_is_always_there() {
        let s = Station::demo();
        assert_eq!(s.name, "demo");
        assert_eq!(s.model, "canned replies");
    }

    #[test]
    fn pick_uses_requested_name() {
        let stations = vec![
            Station::demo(),
            station("a", "m1"),
            station("b", "m2"),
        ];
        let shops = vec![Shop::demo()];
        assert_eq!(pick(&stations, &shops, Some("b")).unwrap().name, "b");
    }

    #[test]
    fn pick_errors_on_unknown_name() {
        let stations = vec![Station::demo()];
        let shops = vec![Shop::demo()];
        assert!(pick(&stations, &shops, Some("nope")).is_err());
    }

    #[test]
    fn pick_synthesizes_from_first_shop_when_no_saved_stations() {
        let stations = vec![Station::demo()];
        let shops = vec![Shop::demo(), shop("kara", &["sonnet", "haiku"])];
        let picked = pick(&stations, &shops, None).unwrap();
        assert_eq!(picked.name, "untitled");
        assert_eq!(picked.model, "sonnet");
    }

    #[test]
    fn pick_falls_back_to_demo_when_nothing_else() {
        let stations = vec![Station::demo()];
        let shops = vec![Shop::demo()];
        let picked = pick(&stations, &shops, None).unwrap();
        assert_eq!(picked.name, "demo");
    }

    #[test]
    fn patience_parses_both_grandma_and_wire_words() {
        assert_eq!(
            PatienceField::Named("quick".into()).into_patience(),
            Some(Patience::Quick)
        );
        assert_eq!(
            PatienceField::Named("LOW".into()).into_patience(),
            Some(Patience::Quick)
        );
        assert_eq!(
            PatienceField::Named("slow".into()).into_patience(),
            Some(Patience::Slow)
        );
        assert_eq!(
            PatienceField::Named("high".into()).into_patience(),
            Some(Patience::Slow)
        );
        assert_eq!(PatienceField::Named("garbage".into()).into_patience(), None);
    }
}
