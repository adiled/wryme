use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ccft::brainrot::{Aggregate, Baseline, bot_score};
use ccft::ledger::Record;
use ccft::lex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ink {
    Brisk,
    Thinning,
    RunningDry,
    Dry,
}

impl Ink {
    pub fn label(self) -> &'static str {
        match self {
            Ink::Brisk => "brisk",
            Ink::Thinning => "thinning",
            Ink::RunningDry => "running dry",
            Ink::Dry => "dry",
        }
    }
}

const THIN_AT: f64 = 0.60;
const RUNNING_DRY_AT: f64 = 0.80;
const DRY_AT: f64 = 0.95;
const MIN_CEILING: u64 = 1_000;
const MAX_CEILING: u64 = 50_000_000;
pub const ROUND_GROWTH_EST: u64 = 1_500;
const CANARY_MIN_RECORDS: usize = 8;
const CANARY_BOT_SCORE: u32 = 70;
const CANARY_STALL_RATIO: f64 = 1.5;
const RECORD_FRACTION: f64 = 0.92;

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
    prod_ink: Ink,
    ink: Ink,
    fill: Option<u64>,
    soft: bool,
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
            prod_ink: Ink::Brisk,
            ink: Ink::Brisk,
            fill: None,
            soft: false,
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
        let record_in = m.record_in;
        self.save();

        self.soft = self.canary();
        self.ink = self.level(ceiling, record_in, self.soft);
        self.fill = match ceiling {
            Some(c) if c > 0 && self.full => Some(((turn_in * 100) / c).min(100)),
            _ => None,
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
            self.ink = Ink::Dry;
            if first {
                return Some(format!(
                    "the ink ran dry at {c} tokens — the book holds everything; \
                     start a fresh window (deem to keep new threads)"
                ));
            }
        }
        let m = self.stations.get(station);
        if m.and_then(|m| m.ceiling).map(|c| self.turn_in >= c * DRY_AT as u64).unwrap_or(false) {
            self.ink = Ink::Dry;
            return Some(
                "the window ran dry — the book holds everything; start a fresh window"
                    .to_string(),
            );
        }
        None
    }

    pub fn take_prod(&mut self) -> Option<String> {
        let ink = self.ink;
        if ink <= self.prod_ink {
            return None;
        }
        self.prod_ink = ink;
        Some(match ink {
            Ink::Thinning => {
                "\n[ink] The ink is thinning — this may be a good moment to deem \
                 the older threads into the book while the window still has room."
                    .to_string()
            }
            Ink::RunningDry => {
                "\n[ink] The window's ink is running dry. Deem this stretch into \
                 the book and draw the turn to a clean close — a fresh window \
                 reopens the book in one step."
                    .to_string()
            }
            _ => {
                "\n[ink] The window is dry — the book holds everything. Bring the \
                 turn to a close; a fresh window reopens your pages in one step."
                    .to_string()
            }
        })
    }

    pub fn dip(&self, station: &str) -> Option<(Ink, Option<u64>)> {
        self.stations.get(station).map(|_| (self.ink, self.fill))
    }

    pub fn loop_guard(&self, station: &str) -> LoopGuard {
        let est = self.stations.get(station).map(|m| {
            m.ceiling
                .unwrap_or_else(|| if m.record_in > 0 { m.record_in * 11 / 10 } else { 0 })
        }).filter(|&e| e > 0);
        LoopGuard { est }
    }

    fn canary(&self) -> bool {
        if self.records.len() < CANARY_MIN_RECORDS {
            return false;
        }
        let a = Aggregate::ingest(self.records.iter().cloned());
        let b = Baseline::from_records(&self.records);
        if bot_score(&a, &b) >= CANARY_BOT_SCORE {
            return true;
        }
        let with_out: Vec<&Record> = self.records.iter().filter(|r| r.out > 0).collect();
        if with_out.len() < 6 {
            return false;
        }
        let mspt = |rs: &[&Record]| {
            let mut v: Vec<f64> = rs.iter().map(|r| r.lat as f64 / r.out as f64).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v[v.len() / 2]
        };
        let (last, prior) = with_out.split_at(with_out.len().saturating_sub(3));
        let (l, p) = (mspt(last), mspt(prior));
        l > CANARY_STALL_RATIO * p && l > 0.0
    }

    fn level(&self, ceiling: Option<u64>, record_in: u64, soft: bool) -> Ink {
        if let Some(c) = ceiling {
            if c > 0 && self.full {
                let f = self.turn_in as f64 / c as f64;
                if f >= DRY_AT {
                    return Ink::Dry;
                }
                if f >= RUNNING_DRY_AT {
                    return Ink::RunningDry;
                }
                if f >= THIN_AT {
                    return Ink::Thinning;
                }
                return Ink::Brisk;
            }
        }
        if soft {
            return Ink::RunningDry;
        }
        if self.turn_in > 0
            && record_in > 0
            && self.turn_in as f64 >= record_in as f64 * RECORD_FRACTION
        {
            return Ink::Thinning;
        }
        Ink::Brisk
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LoopGuard {
    est: Option<u64>,
}

impl LoopGuard {
    pub fn trips(&self, full: bool, last_in: u64, per_round_est: u64) -> bool {
        if !full {
            return false;
        }
        let Some(est) = self.est else {
            return false;
        };
        if last_in == 0 || est == 0 {
            return false;
        }
        last_in + per_round_est > (est as f64 * DRY_AT) as u64
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
    fn phases_follow_the_ceiling() {
        let mut r = Reservoir::default();
        let m = "m1";
        r.station_mut(m).ceiling = Some(100_000);
        r.note_round(m, "m1", 10_000, 100, true);
        r.end_turn("hi there", "", 0);
        assert_eq!(r.ink, Ink::Brisk);
        r.note_round(m, "m1", 65_000, 100, true);
        r.end_turn("b", "", 0);
        assert_eq!(r.ink, Ink::Thinning);
        r.note_round(m, "m1", 90_000, 100, true);
        r.end_turn("c", "", 0);
        assert_eq!(r.ink, Ink::RunningDry);
        r.note_round(m, "m1", 98_000, 100, true);
        r.end_turn("d", "", 0);
        assert_eq!(r.ink, Ink::Dry);
    }

    #[test]
    fn prod_escalates_once_per_band() {
        let mut r = Reservoir::default();
        let m = "m2";
        r.station_mut(m).ceiling = Some(100_000);
        r.note_round(m, "m2", 65_000, 1, true);
        r.end_turn("x", "", 0);
        assert!(r.take_prod().is_some());
        assert!(r.take_prod().is_none());
        r.note_round(m, "m2", 90_000, 1, true);
        r.end_turn("y", "", 0);
        let p = r.take_prod().expect("running dry delivers once");
        assert!(p.contains("running dry"));
        assert!(r.take_prod().is_none());
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

    #[test]
    fn loop_guard_trips_only_near_the_wall() {
        let mut r = Reservoir::default();
        r.station_mut("g").ceiling = Some(100_000);
        r.note_round("g", "g", 94_000, 10, true);
        r.end_turn("test", "", 0);
        let guard = r.loop_guard("g");
        assert!(guard.trips(true, 94_000, 1_500));
        assert!(!guard.trips(true, 50_000, 1_500));
        assert!(!guard.trips(false, 94_000, 1_500));
    }
}