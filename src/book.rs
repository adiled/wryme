#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;

#[derive(Debug, Clone, Default)]
pub struct Bookmark {
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CompartmentMeta {
    pub opened_at: i64,
    pub last_inked: i64,
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
    pub spans: Vec<(u64, u64)>,
    pub life_tokens: i64,
    pub tail: String,
}

#[derive(Debug, Clone)]
pub struct StreamRow {
    pub row_id: u64,
    pub role: String,
    pub content: String,
    pub ts: i64,
    pub window: String,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub ts: i64,
}

pub struct Book {
    dir: PathBuf,
    window: String,
    next_row: u64,
    mine: Vec<u64>,
    unattr_tokens: i64,
    unattr_turns: i64,
    unattr_tail: Vec<(String, String)>,
    index: Vec<CompartmentMeta>,
    pending: Vec<StreamRow>,
    life_summary: String,
}

pub struct Engine {
    pub book: Book,
    pub established: Vec<String>,
    last_deemed: Option<String>,
    pending_prod: bool,
    demanding: bool,
    prod_delivered_row: u64,
    last_lookup_row: u64,
    trouble: Option<String>,
}

pub fn lock(engine: &std::sync::Mutex<Engine>) -> std::sync::MutexGuard<'_, Engine> {
    engine.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod prod {
    pub(super) const MIN_TURNS: i64 = 5;
    pub(super) const MIN_BYTES: i64 = 1600;
    pub(super) const COOLDOWN_TURNS: u64 = 5;
}

mod tail {
    pub(super) const ROWS: usize = 2;
    pub(super) const MAX_CHARS: usize = 400;
    pub(super) const EXCERPT_MAX_CHARS: usize = 240;
}

mod recency {
    const FOUR_HOURS_MS: i64 = 4 * 3_600_000;
    const ONE_DAY_MS: i64 = 24 * 3_600_000;
    const SEVEN_DAYS_MS: i64 = 7 * 24 * 3_600_000;

    pub(super) const BONUS_FOUR_HOURS: i64 = 12;
    pub(super) const BONUS_ONE_DAY: i64 = 6;
    pub(super) const BONUS_SEVEN_DAYS: i64 = 2;

    pub(super) fn bonus(age_ms: i64) -> i64 {
        if age_ms <= FOUR_HOURS_MS {
            BONUS_FOUR_HOURS
        } else if age_ms <= ONE_DAY_MS {
            BONUS_ONE_DAY
        } else if age_ms <= SEVEN_DAYS_MS {
            BONUS_SEVEN_DAYS
        } else {
            0
        }
    }
}

mod term {
    pub(super) const TOPIC_EXACT: i64 = 200;
    pub(super) const TOPIC_WORD: i64 = 10;
    pub(super) const META_WORD: i64 = 3;
    pub(super) const LIGHT_WORD: i64 = 1;
    pub(super) const TAIL_WORD: i64 = 4;
}

pub const FIND_SHOW: usize = 6;

const DEMAND: &str = "\n[bookkeeping] /cls: this conversation is about to leave your \
context. Whatever is not filed in the book now is lost to you. Call the `book` tool with \
action \"deem\" before anything else: choose the page this conversation belongs to (an \
existing one if it fits, otherwise a new topic) and write its tags, people, facts, plans and \
open threads in full, because afterwards you carry on from that page alone. Then reply with \
one short line naming the page.";

pub fn open_engine(dir: &Path, window: &str) -> Result<Engine> {
    let mut book = open_book(dir)?;
    book.window = window.to_string();
    Ok(Engine {
        book,
        established: vec![],
        last_deemed: None,
        demanding: false,
        pending_prod: false,
        last_lookup_row: 0,
        prod_delivered_row: 0,
        trouble: None,
    })
}

impl Engine {
    pub fn take_trouble(&mut self) -> Option<String> {
        self.trouble.take()
    }

    pub fn preamble(&self) -> Vec<String> {
        self.established
            .iter()
            .filter_map(|topic| compartment(&self.book, topic))
            .map(|meta| render_inked(meta, inked_at(meta.last_inked)))
            .collect()
    }

    pub fn take_prod(&mut self) -> Option<String> {
        if !self.pending_prod {
            return None;
        }
        self.pending_prod = false;
        self.prod_delivered_row = self.book.next_row;
        if std::mem::take(&mut self.demanding) {
            return Some(DEMAND.to_string());
        }
        Some(
            "\n[bookkeeping] This stretch of conversation has accumulated \
             enough shape to be written into the book. Look back over it and \
             deem it to the right compartment(s): call the `book` tool with \
             action \"deem\" (action \"new\" first only if no existing \
             compartment fits). This is invisible book-keeping — keep talking \
             to grandma naturally."
                .to_string(),
        )
    }

    pub fn record_turn(&mut self, role: &str, content: &str) {
        if content.is_empty() {
            return;
        }
        let book = &mut self.book;
        book.pending.push(StreamRow {
            row_id: 0,
            role: role.to_string(),
            content: content.to_string(),
            ts: now_ms(),
            window: book.window.clone(),
        });
        book.unattr_tokens += content.len() as i64;
        book.unattr_turns += 1;
        book.unattr_tail
            .push((role.to_string(), content.to_string()));
        if book.unattr_tail.len() > tail::ROWS {
            book.unattr_tail.remove(0);
        }
        if let Err(e) = flush_stream(book) {
            self.trouble = Some(format!("{e:#}"));
        }
        if !self.pending_prod
            && book.unattr_turns >= prod::MIN_TURNS
            && book.unattr_tokens >= prod::MIN_BYTES
            && book.next_row.saturating_sub(self.last_lookup_row) >= prod::COOLDOWN_TURNS
            && book.next_row.saturating_sub(self.prod_delivered_row) >= prod::COOLDOWN_TURNS
        {
            self.pending_prod = true;
        }
    }

    pub fn note_lookup(&mut self) {
        self.last_lookup_row = self.book.next_row;
    }

    pub fn refresh(&mut self) {
        refresh_index(&mut self.book);
    }

    pub fn open(&mut self, topic: &str) -> Result<Option<String>> {
        self.refresh();
        let Some(meta) = compartment(&self.book, topic).cloned() else {
            return Ok(None);
        };
        self.establish(topic);
        self.note_lookup();
        let mut out = render_bookmark(&meta);
        if let Some(thread) = read_compartment(&self.book, topic)? {
            out.push_str("\n\n--- thread ---\n\n");
            out.push_str(&render_compartment(&thread));
        }
        Ok(Some(out))
    }

    pub fn deem(&mut self, bookmark: &Bookmark) -> Result<String> {
        let had = self.unfiled();
        let out = deem_span(&mut self.book, bookmark)?;
        self.pending_prod = false;
        if had && !self.unfiled() {
            self.last_deemed = Some(bookmark.topic.trim().to_string());
        }
        Ok(out)
    }

    pub fn unfiled(&self) -> bool {
        !self.book.mine.is_empty() || !self.book.pending.is_empty()
    }

    pub fn demand_filing(&mut self) {
        self.pending_prod = true;
        self.demanding = true;
    }

    pub fn reopen_last(&mut self) -> Option<String> {
        let topic = self.last_deemed.clone()?;
        self.establish(&topic);
        Some(topic)
    }

    fn establish(&mut self, topic: &str) {
        if !self.established.iter().any(|t| t == topic) {
            self.established.push(topic.to_string());
        }
    }

    pub fn dismiss(&mut self, topic: &str) {
        self.established.retain(|t| t != topic);
    }
}

pub fn open_book(dir: &Path) -> Result<Book> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = dir;
        return Ok(Book {
            dir: dir.to_path_buf(),
            window: String::new(),
            next_row: 0,
            mine: Vec::new(),
            unattr_tokens: 0,
            unattr_turns: 0,
            unattr_tail: Vec::new(),
            index: Vec::new(),
            pending: Vec::new(),
            life_summary: String::new(),
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    open_book_fs(dir)
}

#[cfg(not(target_arch = "wasm32"))]
fn open_book_fs(dir: &Path) -> Result<Book> {
    std::fs::create_dir_all(dir)?;
    let index_path = dir.join("index.parquet");
    let mut index = if index_path.exists() {
        read_index(&index_path)?
    } else {
        vec![]
    };
    index.retain(|m| !m.topic.trim().is_empty());
    let rows = read_stream_on_disk(dir)?;
    let next_row = rows.last().map(|r| r.row_id + 1).unwrap_or(0);

    let summary_path = dir.join("life_summary.txt");
    let life_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();

    for meta in index.iter_mut() {
        meta.tail = meta
            .spans
            .last()
            .map(|(s, e)| format_tail(&tail_entries(&rows, *s, *e)))
            .unwrap_or_default();
    }
    Ok(Book {
        dir: dir.to_path_buf(),
        window: String::new(),
        next_row,
        mine: Vec::new(),
        unattr_tokens: 0,
        unattr_turns: 0,
        unattr_tail: Vec::new(),
        index,
        pending: vec![],
        life_summary,
    })
}

pub fn deem_span(book: &mut Book, bookmark: &Bookmark) -> Result<String> {
    let topic = bookmark.topic.trim().to_string();
    if topic.is_empty() {
        return Ok("a page needs a topic to be deemed into".to_string());
    }
    flush_stream(book)?;
    if book.mine.is_empty() {
        return Ok(format!("no new turns to deem into \"{topic}\""));
    }
    let mut spans: Vec<(u64, u64)> = book.mine.iter().map(|&id| (id, id + 1)).collect();
    merge_spans(&mut spans);
    let now = now_ms();
    let tail = format_tail(&book.unattr_tail);
    let tokens = book.unattr_tokens;
    edit_index(book, |index| {
        let idx = page_for(index, &topic, now);
        let meta = &mut index[idx];
        meta.spans.extend(spans.iter().copied());
        merge_spans(&mut meta.spans);
        meta.life_tokens += tokens;
        for (field, fresh) in [
            (&mut meta.tags, &bookmark.tags),
            (&mut meta.people, &bookmark.people),
            (&mut meta.facts, &bookmark.facts),
            (&mut meta.plans, &bookmark.plans),
            (&mut meta.open, &bookmark.open),
        ] {
            if !fresh.is_empty() {
                *field = fresh.clone();
            }
        }
        meta.last_inked = now;
        meta.tail = tail.clone();
    })?;
    book.mine.clear();
    book.unattr_tokens = 0;
    book.unattr_turns = 0;
    book.unattr_tail.clear();
    let rows = spans
        .iter()
        .map(|(s, e)| format!("{s}..{e}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!("deemed rows {rows} into \"{topic}\"; bookmark refreshed"))
}

fn page_for(index: &mut Vec<CompartmentMeta>, topic: &str, now: i64) -> usize {
    let mut page = None;
    let mut i = 0;
    while i < index.len() {
        if !eq_topic(&index[i].topic, topic) {
            i += 1;
            continue;
        }
        match page {
            None => {
                page = Some(i);
                i += 1;
            }
            Some(p) => {
                let dup = index.remove(i);
                index[p].spans.extend(dup.spans);
                merge_spans(&mut index[p].spans);
                index[p].life_tokens += dup.life_tokens;
                index[p].opened_at = index[p].opened_at.min(dup.opened_at);
            }
        }
    }
    page.unwrap_or_else(|| {
        index.push(CompartmentMeta {
            opened_at: now,
            last_inked: now,
            topic: topic.to_string(),
            tags: vec![],
            people: vec![],
            facts: vec![],
            plans: vec![],
            open: vec![],
            spans: vec![],
            life_tokens: 0,
            tail: String::new(),
        });
        index.len() - 1
    })
}

pub fn update_bookmark(book: &mut Book, topic: &str, bookmark: &Bookmark) -> Result<()> {
    let now = now_ms();
    edit_index(book, |index| {
        if let Some(meta) = index.iter_mut().find(|m| eq_topic(&m.topic, topic)) {
            meta.topic = bookmark.topic.clone();
            meta.tags = bookmark.tags.clone();
            meta.people = bookmark.people.clone();
            meta.facts = bookmark.facts.clone();
            meta.plans = bookmark.plans.clone();
            meta.open = bookmark.open.clone();
            meta.last_inked = now;
        }
    })
}

struct Held(std::fs::File);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn hold(dir: &Path) -> Result<Held> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))?;
    file.lock()?;
    Ok(Held(file))
}

fn carry_tails(fresh: &mut [CompartmentMeta], old: &[CompartmentMeta]) {
    for meta in fresh.iter_mut().filter(|m| m.tail.is_empty()) {
        if let Some(was) = old.iter().find(|o| eq_topic(&o.topic, &meta.topic)) {
            meta.tail = was.tail.clone();
        }
    }
}

fn disk_index(dir: &Path) -> Result<Vec<CompartmentMeta>> {
    let path = dir.join("index.parquet");
    let mut index = if path.exists() { read_index(&path)? } else { vec![] };
    index.retain(|m| !m.topic.trim().is_empty());
    Ok(index)
}

fn edit_index(book: &mut Book, change: impl FnOnce(&mut Vec<CompartmentMeta>)) -> Result<()> {
    let _held = hold(&book.dir)?;
    let mut index = disk_index(&book.dir)?;
    carry_tails(&mut index, &book.index);
    change(&mut index);
    write_index_to(&book.dir, &index)?;
    book.index = index;
    Ok(())
}

fn refresh_index(book: &mut Book) {
    if let Ok(mut index) = disk_index(&book.dir) {
        carry_tails(&mut index, &book.index);
        book.index = index;
    }
}

pub fn index_entries(book: &Book) -> &[CompartmentMeta] {
    &book.index
}

pub fn compartment<'a>(book: &'a Book, topic: &'a str) -> Option<&'a CompartmentMeta> {
    book.index.iter().find(|m| eq_topic(&m.topic, topic))
}

pub fn read_compartment(book: &Book, topic: &str) -> Result<Option<Vec<Message>>> {
    let Some(meta) = compartment(book, topic) else {
        return Ok(None);
    };
    if meta.spans.is_empty() {
        return Ok(Some(vec![]));
    }
    let rows = read_stream(book)?;
    let mut out = Vec::new();
    for (s, e) in &meta.spans {
        for r in &rows {
            if r.row_id >= *s && r.row_id < *e {
                out.push(Message {
                    role: r.role.clone(),
                    content: r.content.clone(),
                    ts: r.ts,
                });
            }
        }
    }
    out.sort_by_key(|m| m.ts);
    Ok(Some(out))
}

pub fn render_bookmark(meta: &CompartmentMeta) -> String {
    render_inked(meta, relative_ago(meta.last_inked))
}

fn render_inked(meta: &CompartmentMeta, inked: String) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", meta.topic));
    if meta.last_inked > 0 {
        out.push_str(&format!("Last inked: {inked}\n"));
    }
    if !meta.people.is_empty() {
        out.push_str(&format!("People: {}\n", meta.people.join(", ")));
    }
    if !meta.facts.is_empty() {
        out.push_str(&format!("Facts: {}\n", meta.facts.join(", ")));
    }
    if !meta.plans.is_empty() {
        out.push_str(&format!("Plans: {}\n", meta.plans.join(", ")));
    }
    if !meta.open.is_empty() {
        out.push_str(&format!("Open: {}\n", meta.open.join(", ")));
    }
    if !meta.tags.is_empty() {
        out.push_str(&format!("Tags: {}\n", meta.tags.join(", ")));
    }
    if !meta.tail.is_empty() {
        out.push_str(&format!("Left off:\n{}\n", meta.tail));
    }
    out
}

pub fn render_compartment(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        out.push_str(&format!("**{}:** {}\n\n", m.role, m.content));
    }
    out
}

pub fn life_summary(book: &Book) -> &str {
    &book.life_summary
}

pub fn set_life_summary(book: &mut Book, summary: &str) -> Result<()> {
    book.life_summary = summary.to_string();
    std::fs::write(book.dir.join("life_summary.txt"), summary)?;
    Ok(())
}

pub fn inked_at(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn relative_ago(ms: i64) -> String {
    let diff = (now_ms() - ms).max(0) / 1000;
    if diff < 60 {
        return "just now".to_string();
    }
    let mins = diff / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 365 {
        return format!("{days}d ago");
    }
    format!("{}y ago", days / 365)
}

pub fn tail_excerpt(meta: &CompartmentMeta) -> String {
    if meta.tail.is_empty() {
        return String::new();
    }
    let mut out: String = meta.tail.chars().take(tail::EXCERPT_MAX_CHARS).collect();
    if meta.tail.chars().count() > tail::EXCERPT_MAX_CHARS {
        out.push('…');
    }
    out
}

fn tail_entries(rows: &[StreamRow], start: u64, end: u64) -> Vec<(String, String)> {
    rows.iter()
        .filter(|r| r.row_id >= start && r.row_id < end)
        .rev()
        .take(tail::ROWS)
        .map(|r| (r.role.clone(), r.content.clone()))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub fn format_tail(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    for (role, content) in entries {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{role}: {content}"));
    }
    if out.chars().count() > tail::MAX_CHARS {
        out = out.chars().take(tail::MAX_CHARS).collect();
        out.push('…');
    }
    out
}

pub fn match_compartments<'a>(book: &'a Book, query: &str) -> Vec<&'a CompartmentMeta> {
    let q = query.trim().to_lowercase();
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return vec![];
    }
    let now = now_ms();
    let mut hits: Vec<(&CompartmentMeta, i64)> = book
        .index
        .iter()
        .filter_map(|m| {
            let joined = |lists: &[Vec<String>]| {
                lists
                    .iter()
                    .map(|l| l.join(" ").to_lowercase())
                    .collect::<Vec<_>>()
            };
            let topic = m.topic.to_lowercase();
            let distilled = joined(&[m.facts.clone(), m.plans.clone(), m.open.clone()]);
            let labels = joined(&[m.tags.clone(), m.people.clone()]);
            let tail = m.tail.to_lowercase();
            let mut score: i64 = 0;
            if topic == q {
                score += term::TOPIC_EXACT;
            }
            for w in &words {
                if topic.contains(w.as_str()) {
                    score += term::TOPIC_WORD;
                }
                if distilled.iter().any(|d| d.contains(w.as_str())) {
                    score += term::META_WORD;
                }
                if tail.contains(w.as_str()) {
                    score += term::TAIL_WORD;
                }
                if labels.iter().any(|l| l.contains(w.as_str())) {
                    score += term::LIGHT_WORD;
                }
            }
            if score > 0 {
                score += recency::bonus(now - m.last_inked);
            }
            (score > 0).then_some((m, score))
        })
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.last_inked.cmp(&a.0.last_inked)));
    hits.into_iter().map(|(m, _)| m).collect()
}

fn flush_stream(book: &mut Book) -> Result<()> {
    if cfg!(target_arch = "wasm32") || book.pending.is_empty() {
        return Ok(());
    }
    let _held = hold(&book.dir)?;
    let counter = book.dir.join("rows");
    let first = match std::fs::read_to_string(&counter).ok().and_then(|t| t.trim().parse::<u64>().ok()) {
        Some(n) => n,
        None => read_stream_on_disk(&book.dir)?.last().map_or(0, |r| r.row_id + 1),
    };
    let next = first + book.pending.len() as u64;
    let staged = book.dir.join(format!("rows.{}", std::process::id()));
    std::fs::write(&staged, next.to_string())?;
    std::fs::rename(&staged, &counter)?;
    for (row, id) in book.pending.iter_mut().zip(first..) {
        row.row_id = id;
    }
    write_stream_segment(&book.dir, first, &book.pending)?;
    book.mine.extend(book.pending.iter().map(|r| r.row_id));
    book.pending.clear();
    book.next_row = next;
    Ok(())
}

fn read_stream_on_disk(dir: &Path) -> Result<Vec<StreamRow>> {
    let mut all = Vec::new();
    let stream_dir = dir.join("stream");
    if let Ok(entries) = std::fs::read_dir(&stream_dir) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "parquet") {
                match read_stream_segment(&path) {
                    Ok(rows) => all.extend(rows),
                    Err(e) => tracing::warn!(path = %path.display(), "skipping unreadable book segment: {e:#}"),
                }
            }
        }
    }
    all.sort_by_key(|r| r.row_id);
    Ok(all)
}

fn read_stream(book: &Book) -> Result<Vec<StreamRow>> {
    let mut all = read_stream_on_disk(&book.dir)?;
    all.extend(book.pending.iter().cloned());
    all.sort_by_key(|r| r.row_id);
    Ok(all)
}

fn eq_topic(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn merge_spans(spans: &mut Vec<(u64, u64)>) {
    spans.sort_by_key(|s| s.0);
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (s, e) in spans.drain(..) {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
            continue;
        }
        merged.push((s, e));
    }
    *spans = merged;
}

fn index_schema() -> SchemaRef {
    let fields = vec![
        Field::new("opened_at", DataType::Int64, false),
        Field::new("last_inked", DataType::Int64, false),
        Field::new("topic", DataType::Utf8, false),
        Field::new("tags", DataType::Utf8, true),
        Field::new("people", DataType::Utf8, true),
        Field::new("facts", DataType::Utf8, true),
        Field::new("plans", DataType::Utf8, true),
        Field::new("open", DataType::Utf8, true),
        Field::new("spans", DataType::Utf8, true),
        Field::new("life_tokens", DataType::Int64, false),
    ];
    Schema::new(fields).into()
}

fn stream_schema() -> SchemaRef {
    let fields = vec![
        Field::new("row_id", DataType::Int64, false),
        Field::new("role", DataType::Utf8, true),
        Field::new("content", DataType::Utf8, true),
        Field::new("ts", DataType::Int64, false),
        Field::new("window", DataType::Utf8, true),
    ];
    Schema::new(fields).into()
}

fn write_index_to(dir: &Path, index: &[CompartmentMeta]) -> Result<()> {
    let opened = Int64Array::from(index.iter().map(|m| m.opened_at).collect::<Vec<_>>());
    let last_inked = Int64Array::from(index.iter().map(|m| m.last_inked).collect::<Vec<_>>());
    let topic = StringArray::from(
        index
            .iter()
            .map(|m| m.topic.as_str())
            .collect::<Vec<_>>(),
    );
    let tags = StringArray::from(
        index
            .iter()
            .map(|m| join(&m.tags))
            .collect::<Vec<String>>(),
    );
    let people = StringArray::from(
        index
            .iter()
            .map(|m| join(&m.people))
            .collect::<Vec<String>>(),
    );
    let facts = StringArray::from(
        index
            .iter()
            .map(|m| join(&m.facts))
            .collect::<Vec<String>>(),
    );
    let plans = StringArray::from(
        index
            .iter()
            .map(|m| join(&m.plans))
            .collect::<Vec<String>>(),
    );
    let open = StringArray::from(
        index
            .iter()
            .map(|m| join(&m.open))
            .collect::<Vec<String>>(),
    );
    let spans = StringArray::from(
        index
            .iter()
            .map(|m| join_spans(&m.spans))
            .collect::<Vec<String>>(),
    );
    let tokens = Int64Array::from(index.iter().map(|m| m.life_tokens).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        index_schema(),
        vec![
            std::sync::Arc::new(opened),
            std::sync::Arc::new(last_inked),
            std::sync::Arc::new(topic),
            std::sync::Arc::new(tags),
            std::sync::Arc::new(people),
            std::sync::Arc::new(facts),
            std::sync::Arc::new(plans),
            std::sync::Arc::new(open),
            std::sync::Arc::new(spans),
            std::sync::Arc::new(tokens),
        ],
    )?;

    let staged = dir.join(format!("index.parquet.{}", std::process::id()));
    let file = std::fs::File::create(&staged)?;
    let mut writer = ArrowWriter::try_new(file, index_schema(), None)?;
    writer.write(&batch)?;
    let _ = writer.close()?;
    std::fs::rename(&staged, dir.join("index.parquet"))?;
    Ok(())
}

fn read_index(path: &Path) -> Result<Vec<CompartmentMeta>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut metas = Vec::new();
    for batch in reader {
        let batch = batch?;
        let opened = ints(&batch, "opened_at")?;
        let last_inked = ints(&batch, "last_inked").or_else(|_| ints(&batch, "updated_at"))?;
        let topic = strs(&batch, "topic")?;
        let tags = strs(&batch, "tags")?;
        let people = strs(&batch, "people")?;
        let facts = strs(&batch, "facts")?;
        let plans = strs(&batch, "plans")?;
        let open = strs(&batch, "open")?;
        let spans = strs(&batch, "spans")?;
        let tokens = ints(&batch, "life_tokens")?;
        for i in 0..batch.num_rows() {
            metas.push(CompartmentMeta {
                opened_at: opened.value(i),
                last_inked: last_inked.value(i),
                topic: topic.value(i).to_string(),
                tags: split(tags.value(i)),
                people: split(people.value(i)),
                facts: split(facts.value(i)),
                plans: split(plans.value(i)),
                open: split(open.value(i)),
                spans: split_spans(spans.value(i)),
                life_tokens: tokens.value(i),
                tail: String::new(),
            });
        }
    }
    Ok(metas)
}

fn write_stream_segment(book_dir: &Path, first: u64, rows: &[StreamRow]) -> Result<()> {
    let dir = book_dir.join("stream");
    std::fs::create_dir_all(&dir)?;
    let ids = Int64Array::from(rows.iter().map(|r| r.row_id as i64).collect::<Vec<_>>());
    let role = StringArray::from(rows.iter().map(|r| r.role.as_str()).collect::<Vec<_>>());
    let content = StringArray::from(rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>());
    let ts = Int64Array::from(rows.iter().map(|r| r.ts).collect::<Vec<_>>());
    let window = StringArray::from(rows.iter().map(|r| r.window.as_str()).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        stream_schema(),
        vec![
            std::sync::Arc::new(ids),
            std::sync::Arc::new(role),
            std::sync::Arc::new(content),
            std::sync::Arc::new(ts),
            std::sync::Arc::new(window),
        ],
    )?;

    let path = dir.join(format!("r{first:08}.parquet"));
    let staged = path.with_extension(format!("parquet.{}", std::process::id()));
    let mut writer = ArrowWriter::try_new(std::fs::File::create(&staged)?, stream_schema(), None)?;
    writer.write(&batch)?;
    writer.into_inner()?.sync_all()?;
    std::fs::rename(&staged, &path)?;
    Ok(())
}

fn read_stream_segment(path: &Path) -> Result<Vec<StreamRow>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let ids = ints(&batch, "row_id")?;
        let role = strs(&batch, "role")?;
        let content = strs(&batch, "content")?;
        let ts = ints(&batch, "ts")?;
        let window = strs(&batch, "window").ok();
        for i in 0..batch.num_rows() {
            rows.push(StreamRow {
                row_id: ids.value(i) as u64,
                role: role.value(i).to_string(),
                content: content.value(i).to_string(),
                ts: ts.value(i),
                window: window.map(|w| w.value(i).to_string()).unwrap_or_default(),
            });
        }
    }
    Ok(rows)
}

fn ints<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>())
        .with_context(|| format!("book column {name} is missing or not a number"))
}

fn strs<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("book column {name} is missing or not text"))
}

fn join(list: &[String]) -> String {
    list.join(",")
}

fn split(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_spans(spans: &[(u64, u64)]) -> String {
    spans
        .iter()
        .map(|(s, e)| format!("{s}:{e}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn split_spans(s: &str) -> Vec<(u64, u64)> {
    s.split(',')
        .filter_map(|t| {
            let (a, b) = t.split_once(':')?;
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        })
        .collect()
}

fn now_ms() -> i64 {
    (crate::platform::unix_secs() * 1000.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wryme_book_{tag}_{}", std::process::id()))
    }

    fn msg(role: &str, content: &str, row_id: u64) -> StreamRow {
        StreamRow {
            row_id,
            role: role.to_string(),
            content: content.to_string(),
            ts: now_ms(),
            window: "test".into(),
        }
    }

    #[test]
    fn stream_is_continuous_and_never_partitioned() {
        let dir = tmpdir("stream");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        book.pending.push(msg("user", "hello", 0));
        book.next_row += 1;
        book.pending.push(msg("assistant", "hi there", 1));
        book.next_row += 1;
        flush_stream(&mut book).unwrap();
        flush_stream(&mut book).unwrap();

        let rows = read_stream(&book).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content, "hello");
        assert_eq!(rows[1].content, "hi there");

        let book2 = open_book(&dir).unwrap();
        assert_eq!(book2.next_row, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deem_attributes_spans_and_multiple_pages_interleave() {
        let dir = tmpdir("deem");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();

        for c in ["soil", "roses", "shade"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(
            &mut book,
            &Bookmark {
                topic: "garden".into(),
                ..Default::default()
            },
        )
        .unwrap();

        for c in ["dog", "fence", "noise"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(
            &mut book,
            &Bookmark {
                topic: "neighbours".into(),
                ..Default::default()
            },
        )
        .unwrap();

        for c in ["compost", "mulch"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(
            &mut book,
            &Bookmark {
                topic: "garden".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let g = compartment(&book, "garden").unwrap();
        let n = compartment(&book, "neighbours").unwrap();
        assert_eq!(g.spans, vec![(0, 3), (6, 8)]);
        assert_eq!(n.spans, vec![(3, 6)]);

        let g_thread = read_compartment(&book, "garden").unwrap().unwrap();
        assert_eq!(g_thread.len(), 5);
        assert_eq!(g_thread[0].content, "soil");
        assert_eq!(g_thread[4].content, "mulch");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_open_page_is_said_the_same_until_inked_again() {
        let dir = tmpdir("preamble");
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = open_engine(&dir, "w1").unwrap();
        engine.record_turn("user", "let us plan the garden");
        engine
            .deem(&Bookmark {
                topic: "garden".into(),
                facts: vec!["roses by the gate".into()],
                ..Default::default()
            })
            .unwrap();
        assert!(
            engine
                .open("garden")
                .unwrap()
                .unwrap()
                .contains("Last inked: just now")
        );
        let page = engine.preamble();
        assert_eq!(page.len(), 1);
        let inked = compartment(&engine.book, "garden").unwrap().last_inked;
        assert!(page[0].contains(&format!("Last inked: {}\n", inked_at(inked))));
        assert!(!page[0].contains("just now") && !page[0].contains(" ago"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deem_takes_this_windows_new_turns_and_skips_empty() {
        let dir = tmpdir("watermark");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();

        assert!(
            deem_span(
                &mut book,
                &Bookmark {
                    topic: "t".into(),
                    ..Default::default()
                }
            )
            .unwrap()
            .contains("no new turns")
        );
        assert!(book.mine.is_empty());

        book.pending.push(msg("user", "one", book.next_row));
        book.next_row += 1;
        book.unattr_turns += 1;
        book.unattr_tokens += 3;
        deem_span(
            &mut book,
            &Bookmark {
                topic: "t".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(book.mine.is_empty());
        assert_eq!(book.unattr_tokens, 0);
        assert_eq!(compartment(&book, "t").unwrap().spans, vec![(0, 1)]);

        assert!(
            deem_span(
                &mut book,
                &Bookmark {
                    topic: "t".into(),
                    ..Default::default()
                }
            )
            .unwrap()
            .contains("no new turns")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_prod_fires_only_when_weightful_and_unshielded() {
        let dir = tmpdir("prod");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir, "w1").unwrap();

        for _ in 0..4 {
            e.record_turn("user", "a");
        }
        assert_eq!(e.take_prod(), None);

        for _ in 0..40 {
            e.record_turn("user", "this is a long enough sentence to count as weight");
        }
        assert!(e.take_prod().is_some());

        let mut e2 = open_engine(&dir, "w2").unwrap();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.note_lookup();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        assert_eq!(e2.take_prod(), None);

        let mut e3 = open_engine(&dir, "w3").unwrap();
        for _ in 0..40 {
            e3.record_turn("user", "this is a long enough sentence to count as weight");
        }
        e3.deem(&Bookmark {
            topic: "x".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(e3.take_prod(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_flushes_every_turn_so_turns_survive_without_deem() {
        let dir = tmpdir("durable");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir, "w1").unwrap();

        e.record_turn("user", "remember our lisbon plans");
        e.record_turn("assistant", "may is lovely");

        let book2 = open_book(&dir).unwrap();
        assert_eq!(book2.next_row, 2);
        let rows = read_stream(&book2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content, "remember our lisbon plans");
        assert_eq!(rows[1].content, "may is lovely");

        let mut e2 = open_engine(&dir, "w2").unwrap();
        e2.record_turn("user", "third turn");
        let book3 = open_book(&dir).unwrap();
        let rows = read_stream(&book3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].content, "third turn");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_ranks_by_tail_content_and_last_inked() {
        let dir = tmpdir("find_rank");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let now = now_ms();

        book.unattr_tail
            .push(("user".into(), "the press loves olive oil".into()));
        book.index.push(CompartmentMeta {
            opened_at: now - 1000,
            last_inked: now - 3_600_000,
            topic: "olive oil".into(),
            tags: vec![],
            people: vec![],
            facts: vec![],
            plans: vec![],
            open: vec![],
            spans: vec![(0, 2)],
            life_tokens: 0,
            tail: format_tail(&book.unattr_tail),
        });
        book.index.push(CompartmentMeta {
            opened_at: now - 2000,
            last_inked: now - 10 * 24 * 3_600_000,
            topic: "olive oil bottles".into(),
            tags: vec![],
            people: vec![],
            facts: vec![],
            plans: vec![],
            open: vec![],
            spans: vec![],
            life_tokens: 0,
            tail: String::new(),
        });

        let hits = match_compartments(&book, "olive oil press");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].topic, "olive oil");
        assert!(tail_excerpt(hits[0]).contains("press loves olive oil"));
        let hits = match_compartments(&book, "bottles");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic, "olive oil bottles");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_book_reads_legacy_updated_at_column() {
        let dir = tmpdir("legacy_col");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("stream")).unwrap();
        let schema = Schema::new(vec![
            Field::new("opened_at", DataType::Int64, false),
            Field::new("updated_at", DataType::Int64, false),
            Field::new("topic", DataType::Utf8, false),
            Field::new("tags", DataType::Utf8, true),
            Field::new("people", DataType::Utf8, true),
            Field::new("facts", DataType::Utf8, true),
            Field::new("plans", DataType::Utf8, true),
            Field::new("open", DataType::Utf8, true),
            Field::new("spans", DataType::Utf8, true),
            Field::new("life_tokens", DataType::Int64, false),
        ]);
        let batch = RecordBatch::try_new(
            std::sync::Arc::new(schema.clone()),
            vec![
                std::sync::Arc::new(Int64Array::from(vec![1])),
                std::sync::Arc::new(Int64Array::from(vec![2])),
                std::sync::Arc::new(StringArray::from(vec!["garden"])),
                std::sync::Arc::new(StringArray::from(vec![Some("roses")])),
                std::sync::Arc::new(StringArray::from(vec![Some("")])),
                std::sync::Arc::new(StringArray::from(vec![Some("soil loves sun")])),
                std::sync::Arc::new(StringArray::from(vec![Some("")])),
                std::sync::Arc::new(StringArray::from(vec![Some("")])),
                std::sync::Arc::new(StringArray::from(vec![Some("0:1")])),
                std::sync::Arc::new(Int64Array::from(vec![10])),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(dir.join("index.parquet")).unwrap();
        let mut writer = ArrowWriter::try_new(file, std::sync::Arc::new(schema), None).unwrap();
        writer.write(&batch).unwrap();
        let _ = writer.close().unwrap();

        let book = open_book(&dir).unwrap();
        let page = compartment(&book, "garden").unwrap();
        assert_eq!(page.last_inked, 2);
        assert_eq!(page.tags, vec!["roses"]);
        assert_eq!(page.facts, vec!["soil loves sun"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rows_on(dir: &Path) -> Vec<StreamRow> {
        read_stream_on_disk(dir).unwrap()
    }

    #[test]
    fn two_windows_never_overwrite_or_share_row_numbers() {
        let dir = tmpdir("two_windows");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = open_engine(&dir, "aaaa").unwrap();
        let mut b = open_engine(&dir, "bbbb").unwrap();
        for i in 0..5 {
            a.record_turn("user", &format!("a{i}"));
            b.record_turn("user", &format!("b{i}"));
        }
        let rows = rows_on(&dir);
        assert_eq!(rows.len(), 10);
        let mut ids: Vec<u64> = rows.iter().map(|r| r.row_id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 10);
        assert!(rows.iter().all(|r| r.content.starts_with(&r.window[..1])));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_torn_turn_file_never_stops_a_window_opening() {
        let dir = tmpdir("torn");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = open_engine(&dir, "aaaa").unwrap();
        a.record_turn("user", "kept");
        std::fs::write(dir.join("stream").join("r00000099.parquet"), b"PAR1 torn").unwrap();
        let mut b = open_engine(&dir, "bbbb").unwrap();
        b.record_turn("user", "after");
        let rows = rows_on(&dir);
        assert_eq!(rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(), ["kept", "after"]);
        assert!(std::fs::read_dir(dir.join("stream")).unwrap().all(|e| {
            e.unwrap().path().extension().is_some_and(|x| x == "parquet")
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_holds_only_the_turns_of_the_window_that_filed_it() {
        let dir = tmpdir("own_turns");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = open_engine(&dir, "aaaa").unwrap();
        let mut b = open_engine(&dir, "bbbb").unwrap();
        for i in 0..3 {
            a.record_turn("user", &format!("about the garden {i}"));
            b.record_turn("user", &format!("about the heart colour {i}"));
        }
        a.deem(&Bookmark { topic: "garden".into(), ..Default::default() }).unwrap();
        let thread = read_compartment(&a.book, "garden").unwrap().unwrap();
        assert_eq!(thread.len(), 3);
        assert!(thread.iter().all(|m| m.content.contains("garden")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pages_filed_by_different_windows_all_survive() {
        let dir = tmpdir("both_pages");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = open_engine(&dir, "aaaa").unwrap();
        let mut b = open_engine(&dir, "bbbb").unwrap();
        a.record_turn("user", "alpha talk");
        b.record_turn("user", "beta talk");
        a.deem(&Bookmark { topic: "alpha".into(), ..Default::default() }).unwrap();
        b.deem(&Bookmark { topic: "beta".into(), ..Default::default() }).unwrap();
        let fresh = open_book(&dir).unwrap();
        assert!(compartment(&fresh, "alpha").is_some());
        assert!(compartment(&fresh, "beta").is_some());
        a.refresh();
        assert!(compartment(&a.book, "beta").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_window_starts_with_nothing_of_anyone_elses() {
        let dir = tmpdir("fresh_tail");
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = open_engine(&dir, "aaaa").unwrap();
        a.record_turn("user", "a secret about the heart");
        let b = open_engine(&dir, "bbbb").unwrap();
        assert!(b.book.unattr_tail.is_empty());
        assert!(b.book.mine.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_turn_files_without_a_window_still_read() {
        let dir = tmpdir("legacy_stream");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("stream")).unwrap();
        let schema = Schema::new(vec![
            Field::new("row_id", DataType::Int64, false),
            Field::new("role", DataType::Utf8, true),
            Field::new("content", DataType::Utf8, true),
            Field::new("ts", DataType::Int64, false),
        ]);
        let batch = RecordBatch::try_new(
            std::sync::Arc::new(schema.clone()),
            vec![
                std::sync::Arc::new(Int64Array::from(vec![41])),
                std::sync::Arc::new(StringArray::from(vec![Some("user")])),
                std::sync::Arc::new(StringArray::from(vec![Some("old words")])),
                std::sync::Arc::new(Int64Array::from(vec![1])),
            ],
        )
        .unwrap();
        let file = std::fs::File::create(dir.join("stream").join("0042.parquet")).unwrap();
        let mut writer = ArrowWriter::try_new(file, std::sync::Arc::new(schema), None).unwrap();
        writer.write(&batch).unwrap();
        let _ = writer.close().unwrap();
        let mut e = open_engine(&dir, "aaaa").unwrap();
        e.record_turn("user", "new words");
        let rows = rows_on(&dir);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].window, "");
        assert_eq!(rows[1].row_id, 42);
        assert_eq!(rows[1].window, "aaaa");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cls_demands_filing_once_then_the_gentle_prod_returns() {
        let dir = tmpdir("demand");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir, "w1").unwrap();
        e.record_turn("user", "plan the garden");
        e.demand_filing();
        let first = e.take_prod().unwrap();
        assert!(first.contains("/cls") && first.contains("lost to you") && first.contains("deem"));
        assert_eq!(e.take_prod(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filing_with_empty_notes_keeps_the_page_notes() {
        let dir = tmpdir("keep_notes");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir, "w1").unwrap();
        e.record_turn("user", "plan the garden");
        e.deem(&Bookmark { topic: "garden".into(), facts: vec!["roses by the gate".into()], ..Default::default() })
            .unwrap();
        e.record_turn("user", "more garden talk");
        e.deem(&Bookmark { topic: "garden".into(), ..Default::default() }).unwrap();
        assert_eq!(compartment(&e.book, "garden").unwrap().facts, vec!["roses by the gate"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_without_notes_still_says_where_it_left_off() {
        let dir = tmpdir("left_off");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir, "w1").unwrap();
        e.record_turn("user", "so whats the closest price we got to?");
        e.record_turn("assistant", "eggs were about four a dozen");
        e.deem(&Bookmark { topic: "egg rap".into(), ..Default::default() }).unwrap();
        e.reopen_last();
        let page = e.preamble().join("\n");
        assert!(page.contains("# egg rap"));
        assert!(page.contains("Left off:\nuser: so whats the closest price we got to?"));
        assert!(page.contains("assistant: eggs were about four a dozen"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
