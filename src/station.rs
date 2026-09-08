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
    /// Speech voice for read-aloud replies (Ctrl-V). Heard, never sent:
    /// e.g. "Zarvox" via `say`, any espeak voice via `spd-say`.
    pub voice: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Dials {
    /// Temperature. 0.0 to 2.0, conventionally. Unset = let the model
    /// pick its own default.
    pub boldness: Option<f32>,
    /// Reasoning effort. Defaults to steady (medium). Only meaningful
    /// on models that support extended thinking. Translated on the wire
    /// to Responses `reasoning.effort` and Chat `reasoning_effort`.
    pub patience: Option<Patience>,
    /// Max output tokens. Hard ceiling on reply length. Unset = let the
    /// model stop when it thinks it is done.
    pub verbosity: Option<u32>,
    /// Tinker keep: how many tool pairs survive in replay. All = keep everything.
    pub tinker_keep: TinkerKeep,
    /// Tinker clip: how much of each tool result body survives. Full = verbatim.
    pub tinker_clip: TinkerClip,
}

impl Default for Dials {
    fn default() -> Self {
        Self {
            boldness: None,
            patience: None,
            verbosity: None,
            tinker_keep: TinkerVal::All,
            tinker_clip: TinkerVal::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinkerVal {
    All,
    Count(usize),
    Percent(u8),
}

pub type TinkerKeep = TinkerVal;
pub type TinkerClip = TinkerVal;

impl TinkerVal {
    pub fn keep_n(self, total: usize) -> Option<usize> {
        match self {
            TinkerVal::All => None,
            TinkerVal::Count(n) => Some(n.min(total)),
            TinkerVal::Percent(p) => {
                let p = (p as usize).min(100);
                Some(((total * p).div_ceil(100)).min(total))
            }
        }
    }
    pub fn clip(self, s: &str) -> String {
        match self {
            TinkerVal::All => s.to_string(),
            TinkerVal::Count(0) => String::new(),
            TinkerVal::Count(n) => crate::api::truncate(s, n),
            TinkerVal::Percent(0) => String::new(),
            TinkerVal::Percent(p) => {
                let keep = (s.len() * p as usize).div_ceil(100);
                crate::api::truncate(s, keep)
            }
        }
    }
    pub fn label(self) -> String {
        match self {
            TinkerVal::All => "all".to_string(),
            TinkerVal::Count(n) => n.to_string(),
            TinkerVal::Percent(p) => format!("{}%", p),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patience {
    Bare,
    Swift,
    Quick,
    Steady,
    Slow,
    Deep,
    Max,
}

impl Patience {
    pub fn as_wire(self) -> &'static str {
        match self {
            Patience::Bare => "none",
            Patience::Swift => "minimal",
            Patience::Quick => "low",
            Patience::Steady => "medium",
            Patience::Slow => "high",
            Patience::Deep => "xhigh",
            Patience::Max => "max",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Patience::Bare => "bare",
            Patience::Swift => "swift",
            Patience::Quick => "quick",
            Patience::Steady => "steady",
            Patience::Slow => "slow",
            Patience::Deep => "deep",
            Patience::Max => "max",
        }
    }
}

impl Station {
    pub fn demo() -> Self {
        Self {
            name: "demo".into(),
            model: "canned replies".into(),
            dials: Dials::default(),
            voice: None,
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
    #[serde(default)]
    tinker_keep: Option<TinkerField>,
    #[serde(default)]
    tinker_clip: Option<TinkerField>,
    #[serde(default)]
    voice: Option<String>,
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
            "bare" | "none" => Some(Patience::Bare),
            "swift" | "minimal" => Some(Patience::Swift),
            "quick" | "low" => Some(Patience::Quick),
            "steady" | "medium" => Some(Patience::Steady),
            "slow" | "high" => Some(Patience::Slow),
            "deep" | "xhigh" => Some(Patience::Deep),
            "max" => Some(Patience::Max),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TinkerField {
    Integer(i64),
    Named(String),
}

impl TinkerField {
    fn into_val(self) -> Option<TinkerVal> {
        match self {
            TinkerField::Integer(n) if n >= 0 => Some(TinkerVal::Count(n as usize)),
            TinkerField::Named(s) => {
                let s = s.trim().to_lowercase();
                if s == "all" {
                    return Some(TinkerVal::All);
                }
                if let Some(pct) = s.strip_suffix('%') {
                    if let Ok(p) = pct.trim().parse::<u8>() {
                        if p <= 100 {
                            return Some(TinkerVal::Percent(p));
                        }
                    }
                }
                if let Ok(n) = s.parse::<usize>() {
                    return Some(TinkerVal::Count(n));
                }
                None
            }
            _ => None,
        }
    }
}

impl StationDef {
    fn resolve(self) -> Station {
        let mut dials = Dials::default();
        if self.boldness.is_some() {
            dials.boldness = self.boldness;
        }
        if self.patience.is_some() {
            dials.patience = self.patience.and_then(|p| p.into_patience());
        }
        if self.verbosity.is_some() {
            dials.verbosity = self.verbosity;
        }
        if let Some(k) = self.tinker_keep.and_then(|k| k.into_val()) {
            dials.tinker_keep = k;
        }
        if let Some(c) = self.tinker_clip.and_then(|c| c.into_val()) {
            dials.tinker_clip = c;
        }
        Station {
            name: self.name,
            model: self.model,
            dials,
            voice: self.voice,
        }
    }
}

pub fn load_all() -> Result<Vec<Station>> {
    let mut out = vec![Station::demo()];

    if let Some(env_st) = from_env() {
        out.push(env_st);
    }

    if let Some(path) = config_path() {
        if !path.exists() {
            let _ = ensure_default_file(&path);
        }
        if path.exists() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let parsed: StationsFile =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            for def in parsed.station {
                out.push(def.resolve());
            }
        }
    }
    Ok(out)
}

fn ensure_default_file(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = r#"# wryme stations — canned is local, no network. All dials shown with defaults.
[[station]]
name = "canned"
model = "canned replies"
# boldness = 0.7
patience = "steady"
# verbosity = 8000
tinker_keep = "all"
tinker_clip = "all"
# voice = "Tara"
"#;
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn from_env() -> Option<Station> {
    let model = std::env::var("WME_DEFAULT_STATION_MODEL").ok()?;
    let name = std::env::var("WME_DEFAULT_STATION_NAME").unwrap_or_else(|_| "default".into());
    Some(Station {
        name,
        model,
        dials: Dials::default(),
        voice: None,
    })
}

pub(crate) fn config_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("wryme")
            .join("stations.toml")
    })
}

/// Pick the active station given the loaded list, the loaded shops, and
/// an optional explicit name. Returns the station plus its "origin": the
/// name of the saved entry this session traces back to, or None if the
/// station was synthesized from scratch (untitled or demo).
pub fn pick(
    stations: &[Station],
    shops: &[Shop],
    requested: Option<&str>,
) -> Result<(Station, Option<String>)> {
    if let Some(name) = requested {
        let found = stations.iter().find(|s| s.name == name).cloned();
        return found
            .map(|s| {
                let origin = if s.name == "demo" {
                    None
                } else {
                    Some(s.name.clone())
                };
                (s, origin)
            })
            .with_context(|| {
                let known: Vec<&str> = stations.iter().map(|s| s.name.as_str()).collect();
                format!("no station named '{}'. known: {}", name, known.join(", "))
            });
    }
    // Prefer the first non-demo station the user has saved.
    if let Some(st) = stations.iter().find(|s| s.name != "demo") {
        return Ok((st.clone(), Some(st.name.clone())));
    }
    // No saved stations. Synthesize one from the first non-demo shop's
    // first advertised model. Convention says that is the newest.
    if let Some(shop) = shops.iter().find(|s| s.name != "demo") {
        if let Some(model) = shop.models.first() {
            return Ok((
                Station {
                    name: "untitled".into(),
                    model: model.clone(),
                    dials: Dials::default(),
                    voice: None,
                },
                None,
            ));
        }
    }
    // Nothing configured at all. Demo.
    Ok((Station::demo(), None))
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
            window: crate::shop::WindowMode::Full,
            models: models.iter().map(|s| s.to_string()).collect(),
            headers: std::collections::HashMap::new(),
        }
    }

    fn station(name: &str, model: &str) -> Station {
        Station {
            name: name.into(),
            model: model.into(),
            dials: Dials::default(),
            voice: None,
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
        let stations = vec![Station::demo(), station("a", "m1"), station("b", "m2")];
        let shops = vec![Shop::demo()];
        let (got, origin) = pick(&stations, &shops, Some("b")).unwrap();
        assert_eq!(got.name, "b");
        assert_eq!(origin.as_deref(), Some("b"));
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
        let (got, origin) = pick(&stations, &shops, None).unwrap();
        assert_eq!(got.name, "untitled");
        assert_eq!(got.model, "sonnet");
        // Synthesized: no origin.
        assert_eq!(origin, None);
    }

    #[test]
    fn pick_falls_back_to_demo_when_nothing_else() {
        let stations = vec![Station::demo()];
        let shops = vec![Shop::demo()];
        let (got, origin) = pick(&stations, &shops, None).unwrap();
        assert_eq!(got.name, "demo");
        assert_eq!(origin, None);
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
        assert_eq!(
            PatienceField::Named("bare".into()).into_patience(),
            Some(Patience::Bare)
        );
        assert_eq!(
            PatienceField::Named("minimal".into()).into_patience(),
            Some(Patience::Swift)
        );
        assert_eq!(
            PatienceField::Named("deep".into()).into_patience(),
            Some(Patience::Deep)
        );
        assert_eq!(
            PatienceField::Named("max".into()).into_patience(),
            Some(Patience::Max)
        );
        assert_eq!(PatienceField::Named("garbage".into()).into_patience(), None);
    }
}
