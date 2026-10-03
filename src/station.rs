use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

use crate::shop::Shop;

pub const DEMO: &str = "demo";
const UNTITLED: &str = "untitled";

#[derive(Debug, Clone)]
pub struct Station {
    pub name: String,
    pub model: String,
    pub dials: Dials,
    pub voice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dials {
    pub boldness: Option<f32>,
    pub patience: Option<Patience>,
    pub brainy: Option<Brainy>,
    pub tinker_keep: TinkerKeep,
    pub tinker_clip: TinkerClip,
}

impl Dials {
    pub fn thinking_hidden(&self) -> bool {
        self.brainy == Some(Brainy::Hush)
    }
}

impl Default for Dials {
    fn default() -> Self {
        Self {
            boldness: None,
            patience: None,
            brainy: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brainy {
    Hush,
    Murmur,
    Chatty,
    Gabby,
}

impl Brainy {
    pub fn as_wire(self) -> &'static str {
        match self {
            Brainy::Hush => "none",
            Brainy::Murmur => "concise",
            Brainy::Chatty => "auto",
            Brainy::Gabby => "detailed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Brainy::Hush => "hush",
            Brainy::Murmur => "murmur",
            Brainy::Chatty => "chatty",
            Brainy::Gabby => "gabby",
        }
    }
}

impl Station {
    pub fn demo() -> Self {
        Self {
            name: DEMO.into(),
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
    brainy: Option<BrainyField>,
    #[serde(default)]
    tinker_keep: Option<TinkerField>,
    #[serde(default)]
    tinker_clip: Option<TinkerField>,
    #[serde(default)]
    voice: Option<String>,
}

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
enum BrainyField {
    Named(String),
}

impl BrainyField {
    fn into_brainy(self) -> Option<Brainy> {
        let BrainyField::Named(s) = self;
        match s.to_lowercase().as_str() {
            "hush" | "none" => Some(Brainy::Hush),
            "murmur" | "concise" => Some(Brainy::Murmur),
            "chatty" | "auto" => Some(Brainy::Chatty),
            "gabby" | "detailed" => Some(Brainy::Gabby),
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
                if let Some(pct) = s.strip_suffix('%')
                    && let Ok(p) = pct.trim().parse::<u8>()
                    && p <= 100
                {
                    return Some(TinkerVal::Percent(p));
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
        if self.brainy.is_some() {
            dials.brainy = self.brainy.and_then(|b| b.into_brainy());
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
# brainy = "murmur"
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

pub fn pick(
    stations: &[Station],
    shops: &[Shop],
    requested: Option<&str>,
) -> Result<(Station, Option<String>)> {
    if let Some(name) = requested {
        let found = stations.iter().find(|s| s.name == name).cloned();
        return found
            .map(|s| {
                let origin = if s.name == DEMO {
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
    if let Some(st) = stations.iter().find(|s| s.name != DEMO) {
        return Ok((st.clone(), Some(st.name.clone())));
    }
    if let Some(shop) = shops.iter().find(|s| s.name != DEMO)
        && let Some(model) = shop.models.first()
    {
        return Ok((
            Station {
                name: UNTITLED.into(),
                model: model.clone(),
                dials: Dials::default(),
                voice: None,
            },
            None,
        ));
    }
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
        let shops = vec![Shop::demo(), shop("grrrr", &["bhow", "quack"])];
        let (got, origin) = pick(&stations, &shops, None).unwrap();
        assert_eq!(got.name, "untitled");
        assert_eq!(got.model, "bhow");
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

    #[test]
    fn brainy_parses_both_grandma_and_wire_words() {
        assert_eq!(
            BrainyField::Named("hush".into()).into_brainy(),
            Some(Brainy::Hush)
        );
        assert_eq!(
            BrainyField::Named("none".into()).into_brainy(),
            Some(Brainy::Hush)
        );
        assert_eq!(
            BrainyField::Named("MURMUR".into()).into_brainy(),
            Some(Brainy::Murmur)
        );
        assert_eq!(
            BrainyField::Named("concise".into()).into_brainy(),
            Some(Brainy::Murmur)
        );
        assert_eq!(
            BrainyField::Named("chatty".into()).into_brainy(),
            Some(Brainy::Chatty)
        );
        assert_eq!(
            BrainyField::Named("auto".into()).into_brainy(),
            Some(Brainy::Chatty)
        );
        assert_eq!(
            BrainyField::Named("gabby".into()).into_brainy(),
            Some(Brainy::Gabby)
        );
        assert_eq!(
            BrainyField::Named("detailed".into()).into_brainy(),
            Some(Brainy::Gabby)
        );
        assert_eq!(BrainyField::Named("garbage".into()).into_brainy(), None);
    }

    #[test]
    fn brainy_words_reach_the_wire() {
        assert_eq!(Brainy::Hush.as_wire(), "none");
        assert_eq!(Brainy::Murmur.as_wire(), "concise");
        assert_eq!(Brainy::Chatty.as_wire(), "auto");
        assert_eq!(Brainy::Gabby.as_wire(), "detailed");
    }

    #[test]
    fn only_hush_hides_thinking() {
        let mut d = Dials::default();
        assert!(!d.thinking_hidden());
        d.brainy = Some(Brainy::Gabby);
        assert!(!d.thinking_hidden());
        d.brainy = Some(Brainy::Murmur);
        assert!(!d.thinking_hidden());
        d.brainy = Some(Brainy::Chatty);
        assert!(!d.thinking_hidden());
        d.brainy = Some(Brainy::Hush);
        assert!(d.thinking_hidden());
    }
}
