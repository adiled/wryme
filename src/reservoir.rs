use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ccft::brainrot::{Aggregate, Baseline, bot_score};
use ccft::ledger::Record;
use ccft::lex;

const DRY_AT: f64 = 0.95;
const MIN_CEILING: u64 = 1_000;
const MAX_CEILING: u64 = 50_000_000;
const STATIC_MIN_RECORDS: usize = 4;
pub const STATIC_BOT_SCORE: u32 = 70;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StationInk {
    pub ceiling: Option<u64>,
    pub record_in: u64,
    pub turns: u64,
}

#[derive(Debug)]
pub struct Reservoir {
    path: PathBuf,
    stations: HashMap<String, StationInk>,
    records: Vec<Record>,
    seen: HashSet<String>,
    start_time: Option<SystemTime>,
    start: Option<Instant>,
    turn_station: Option<String>,
    turn_model: Option<String>,
    turn_in: u64,
    turn_out: u64,
    full: bool,
    fill: f64,
    static_score: u32,
}

impl Default for Reservoir {
    fn default() -> Self {
        Self {
            path: reservoir_path(),
            stations: HashMap::new(),
            records: Vec::new(),
            seen: HashSet::new(),
            start_time: None,
            start: None,
            turn_station: None,
            turn_model: None,
            turn_in: 0,
            turn_out: 0,
            full: true,
            fill: 0.0,
            static_score: 0,
        }
    }
}

fn reservoir_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("wryme")
        .join("reservoir.json")
}

impl Reservoir {
    pub fn load() -> Self {
        let path = reservoir_path();
        let mut r = Self::default();
        r.path = path;
        if let Ok(text) = std::fs::read_to_string(&r.path) {
            if let Ok(stations) = serde_json::from_str::<HashMap<String, StationInk>>(&text) {
                r.stations = stations;
            }
        }
        r
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all(self.path.parent().unwrap_or(&self.path));
        if let Ok(text) = serde_json::to_string(&self.stations) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    fn station_mut(&mut self, name: &str) -> &mut StationInk {
        self.stations.entry(name.to_string()).or_default()
    }

    pub fn turn_started(&mut self) {
        self.start_time = Some(SystemTime::now());
        self.start = Some(Instant::now());
        self.turn_station = None;
        self.turn_model = None;
        self.turn_in = 0;
        self.turn_out = 0;
        self.fill = 0.0;
    }

    pub fn note_round(&mut self, station: &str, model: &str, input: u64, output: u64, full: bool) {
        self.full = full;
        self.turn_station = Some(station.to_string());
        self.turn_model = Some(model.to_string());
        self.turn_in = input;
        self.turn_out += output;
    }

    pub fn end_turn(&mut self, user_text: &str, brain_text: &str, tool_chars: u64) {
        if self.turn_in == 0 && self.turn_out == 0 {
            return;
        }
        let Some(station) = self.turn_station.clone() else {
            return;
        };
        let model = self.turn_model.clone().unwrap_or_default();
        let now = now_secs();
        let ts = self
            .start_time
            .map(|t| {
                t.duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(now)
            })
            .unwrap_or(now);
        let lat = self
            .start
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let (ttr, fnw, nge) = lex::lexical_stats(user_text);
        let (nvt, fresh) = lex::novelty_fraction(user_text, Some(&self.seen));
        self.seen.extend(fresh);

        let record = Record {
            ts,
            te: now,
            model: Some(model.clone()),
            sid: None,
            r#in: self.turn_in,
            out: self.turn_out,
            tot: self.turn_in + self.turn_out,
            lat,
            cr: 0,
            cc: 0,
            c_us: None,
            u_ch: user_text.chars().count().min(5000) as u64,
            tr_ch: tool_chars,
            th_ch: brain_text.chars().count() as u64,
            reference: None,
            lex_div: ttr,
            fn_word_frac: fnw,
            ngram_entropy: nge,
            nvt,
        };
        self.records.push(record);
        if self.records.len() > 512 {
            self.records.remove(0);
        }

        let turn_in = self.turn_in;
        let m = self.station_mut(&station);
        m.turns += 1;
        if turn_in > m.record_in {
            m.record_in = turn_in;
        }
        let ceiling = m.ceiling;
        self.save();

        self.refresh_static();
        self.fill = match ceiling {
            Some(c) if c > 0 && self.full => (turn_in as f64 / c as f64).min(1.0),
            _ => 0.0,
        };
        self.turn_station = None;
        self.turn_model = None;
    }

    pub fn note_error(&mut self, station: &str, message: &str) -> Option<String> {
        let learned = learn_ceiling(message);
        if let Some(c) = learned {
            let m = self.station_mut(station);
            let first = m.ceiling.is_none();
            m.ceiling = Some(m.ceiling.map_or(c, |old| old.min(c)));
            self.save();
            self.fill = 1.0;
            if first {
                return Some(format!(
                    "the ink ran dry at {c} tokens — the book holds everything; \
                     start a fresh window (deem to keep new threads)"
                ));
            }
        }
        let m = self.stations.get(station);
        if m.and_then(|m| m.ceiling).map(|c| self.turn_in >= c * DRY_AT as u64).unwrap_or(false) {
            self.fill = 1.0;
            return Some(
                "the window ran dry — the book holds everything; start a fresh window"
                    .to_string(),
            );
        }
        None
    }

    pub fn dip(&self, station: &str) -> Option<f64> {
        self.stations.get(station).map(|_| self.fill)
    }

    pub fn static_figure(&self) -> u32 {
        self.static_score
    }

    fn refresh_static(&mut self) {
        if self.records.len() < STATIC_MIN_RECORDS {
            self.static_score = 0;
            return;
        }
        let a = Aggregate::ingest(self.records.iter().cloned());
        let b = Baseline::from_records(&self.records);
        self.static_score = bot_score(&a, &b);
    }
}

fn learn_ceiling(msg: &str) -> Option<u64> {
    let lower = msg.to_lowercase();
    let bytes = lower.as_bytes();
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    for w in ["maximum", "max", "context", "window", "limit"] {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(w) {
            let i = from + rel;
            anchors.push((
                i.saturating_sub(40),
                (i + w.len() + 40).min(lower.len()),
            ));
            from = i + w.len();
        }
    }
    let mut candidates: Vec<u64> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if anchors.iter().any(|&(lo, hi)| start >= lo && start < hi) {
                if let Ok(n) = lower[start..i].parse::<u64>() {
                    if (MIN_CEILING..=MAX_CEILING).contains(&n) {
                        candidates.push(n);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    candidates.into_iter().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceiling_parses_openai_shape() {
        let m = "This model's maximum context length is 200000 tokens. However, \
                 you requested 203017 tokens (201017 in the messages, 2000 in the \
                 functions). Reduce the length of the messages.";
        assert_eq!(learn_ceiling(m), Some(200000));
    }

    #[test]
    fn ceiling_parses_total_request_shape() {
        let m = "The total request size (203020) exceeds the model's maximum \
                 context window (131072 tokens).";
        assert_eq!(learn_ceiling(m), Some(131072));
    }

    #[test]
    fn ceiling_ignores_small_and_far_numbers() {
        let m = "status 400, id abcd1234";
        assert_eq!(learn_ceiling(m), None);
        let m = "an error 7500 happened somewhere deep in the log";
        assert_eq!(learn_ceiling(m), None);
    }

    #[test]
    fn record_in_tracks_max_prompt() {
        let mut r = Reservoir::default();
        r.note_round("m3", "m3", 500, 10, true);
        r.end_turn("a b c d e f g h i j k l m n o p q r s t", "", 0);
        r.note_round("m3", "m3", 1200, 10, true);
        r.end_turn("more words for balance here today", "", 0);
        assert_eq!(r.stations["m3"].record_in, 1200);
    }

    #[test]
    fn empty_turns_leave_no_record() {
        let mut r = Reservoir::default();
        r.turn_started();
        r.end_turn("", "", 0);
        assert!(r.records.is_empty());
    }
}