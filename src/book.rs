#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;
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
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub ts: i64,
}

pub struct Book {
    dir: PathBuf,
    next_row: u64,
    next_seg: u64,
    watermark: u64,
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
    pending_prod: bool,
    prod_delivered_row: u64,
    last_lookup_row: u64,
}

const WEIGHT_TURNS: i64 = 5;
const WEIGHT_BYTES: i64 = 1600;
const LOOKUP_GAP: u64 = 5;

const TAIL_ROWS: usize = 2;
const TAIL_MAX_CHARS: usize = 400;
const EXCERPT_MAX_CHARS: usize = 240;
const RECENT_4H_MS: i64 = 4 * 3_600_000;
const RECENT_1D_MS: i64 = 24 * 3_600_000;
const RECENT_7D_MS: i64 = 7 * 24 * 3_600_000;
const BONUS_RECENT_4H: i64 = 12;
const BONUS_RECENT_1D: i64 = 6;
const BONUS_RECENT_7D: i64 = 2;
const WEIGHT_TOPIC_EXACT: i64 = 200;
const WEIGHT_TOPIC_WORD: i64 = 10;
const WEIGHT_META_WORD: i64 = 3;
const WEIGHT_LIGHT_WORD: i64 = 1;
const WEIGHT_TAIL_WORD: i64 = 4;
pub const FIND_SHOW: usize = 6;

pub fn open_engine(dir: &Path) -> Result<Engine> {
    Ok(Engine {
        book: open_book(dir)?,
        established: vec![],
        pending_prod: false,
        last_lookup_row: 0,
        prod_delivered_row: 0,
    })
}

impl Engine {
    pub fn preamble(&self) -> Vec<String> {
        self.established
            .iter()
            .filter_map(|topic| compartment(&self.book, topic))
            .map(render_bookmark)
            .collect()
    }

    pub fn take_prod(&mut self) -> Option<String> {
        if !self.pending_prod {
            return None;
        }
        self.pending_prod = false;
        self.prod_delivered_row = self.book.next_row;
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
            row_id: book.next_row,
            role: role.to_string(),
            content: content.to_string(),
            ts: now_ms(),
        });
        book.next_row += 1;
        book.unattr_tokens += content.len() as i64;
        book.unattr_turns += 1;
        book.unattr_tail
            .push((role.to_string(), content.to_string()));
        if book.unattr_tail.len() > TAIL_ROWS {
            book.unattr_tail.remove(0);
        }
        let _ = flush_stream(book);
        if !self.pending_prod
            && book.unattr_turns >= WEIGHT_TURNS
            && book.unattr_tokens >= WEIGHT_BYTES
            && book.next_row.saturating_sub(self.last_lookup_row) >= LOOKUP_GAP
            && book.next_row.saturating_sub(self.prod_delivered_row) >= LOOKUP_GAP
        {
            self.pending_prod = true;
        }
    }

    pub fn note_lookup(&mut self) {
        self.last_lookup_row = self.book.next_row;
    }

    pub fn open(&mut self, topic: &str) -> Result<Option<String>> {
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
        let out = deem_span(&mut self.book, bookmark)?;
        self.pending_prod = false;
        Ok(out)
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
    let next_seg = read_stream_max_seg(dir)? + 1;

    let summary_path = dir.join("life_summary.txt");
    let life_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();

    let watermark = index
        .iter()
        .flat_map(|m| m.spans.iter().map(|(_, e)| *e))
        .max()
        .unwrap_or(0)
        .min(next_row);

    for meta in index.iter_mut() {
        meta.tail = meta
            .spans
            .last()
            .map(|(s, e)| format_tail(&tail_entries(&rows, *s, *e)))
            .unwrap_or_default();
    }
    let unattr_tail = tail_entries(&rows, watermark, next_row);

    Ok(Book {
        dir: dir.to_path_buf(),
        next_row,
        next_seg,
        watermark,
        unattr_tokens: 0,
        unattr_turns: 0,
        unattr_tail,
        index,
        pending: vec![],
        life_summary,
    })
}

pub fn deem_span(book: &mut Book, bookmark: &Bookmark) -> Result<String> {
    let topic = bookmark.topic.trim();
    if topic.is_empty() {
        return Ok("a page needs a topic to be deemed into".to_string());
    }
    let now = now_ms();
    let mut page = None;
    let mut i = 0;
    while i < book.index.len() {
        if eq_topic(&book.index[i].topic, topic) {
            if page.is_none() {
                page = Some(i);
                i += 1;
                continue;
            }
            let dup = book.index.remove(i);
            if let Some(p) = page {
                book.index[p].spans.extend(dup.spans);
                merge_spans(&mut book.index[p].spans);
                book.index[p].life_tokens += dup.life_tokens;
                book.index[p].opened_at = book.index[p].opened_at.min(dup.opened_at);
            }
            continue;
        }
        i += 1;
    }
    let idx = match page {
        Some(i) => i,
        None => {
            book.index.push(CompartmentMeta {
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
            book.index.len() - 1
        }
    };
    let start = book.watermark;
    let end = book.next_row;
    if start >= end {
        return Ok(format!("no new turns to deem into \"{topic}\""));
    }
    let meta = &mut book.index[idx];
    meta.spans.push((start, end));
    merge_spans(&mut meta.spans);
    meta.life_tokens += book.unattr_tokens;
    meta.tags = bookmark.tags.clone();
    meta.people = bookmark.people.clone();
    meta.facts = bookmark.facts.clone();
    meta.plans = bookmark.plans.clone();
    meta.open = bookmark.open.clone();
    meta.last_inked = now;
    meta.tail = format_tail(&book.unattr_tail);
    flush_stream(book)?;
    book.watermark = end;
    book.unattr_tokens = 0;
    book.unattr_turns = 0;
    book.unattr_tail.clear();
    write_index(book)?;
    Ok(format!(
        "deemed rows {start}..{end} into \"{topic}\"; bookmark refreshed"
    ))
}

pub fn update_bookmark(book: &mut Book, topic: &str, bookmark: &Bookmark) -> Result<()> {
    let Some(meta) = book.index.iter_mut().find(|m| eq_topic(&m.topic, topic)) else {
        return Ok(());
    };
    meta.topic = bookmark.topic.clone();
    meta.tags = bookmark.tags.clone();
    meta.people = bookmark.people.clone();
    meta.facts = bookmark.facts.clone();
    meta.plans = bookmark.plans.clone();
    meta.open = bookmark.open.clone();
    meta.last_inked = now_ms();
    write_index(book)?;
    Ok(())
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
    let mut out = String::new();
    out.push_str(&format!("# {}\n", meta.topic));
    if meta.last_inked > 0 {
        out.push_str(&format!("Last inked: {}\n", relative_ago(meta.last_inked)));
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
    let mut out: String = meta.tail.chars().take(EXCERPT_MAX_CHARS).collect();
    if meta.tail.chars().count() > EXCERPT_MAX_CHARS {
        out.push('…');
    }
    out
}

fn tail_entries(rows: &[StreamRow], start: u64, end: u64) -> Vec<(String, String)> {
    rows.iter()
        .filter(|r| r.row_id >= start && r.row_id < end)
        .rev()
        .take(TAIL_ROWS)
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
    if out.chars().count() > TAIL_MAX_CHARS {
        out = out.chars().take(TAIL_MAX_CHARS).collect();
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
            let topic = m.topic.to_lowercase();
            let facts = m.facts.join(" ").to_lowercase();
            let plans = m.plans.join(" ").to_lowercase();
            let open = m.open.join(" ").to_lowercase();
            let tags = m.tags.join(" ").to_lowercase();
            let people = m.people.join(" ").to_lowercase();
            let tail = m.tail.to_lowercase();
            let mut score: i64 = 0;
            if topic == q {
                score += WEIGHT_TOPIC_EXACT;
            }
            for w in &words {
                if topic.contains(w.as_str()) {
                    score += WEIGHT_TOPIC_WORD;
                }
                if facts.contains(w.as_str()) {
                    score += WEIGHT_META_WORD;
                }
                if plans.contains(w.as_str()) {
                    score += WEIGHT_META_WORD;
                }
                if open.contains(w.as_str()) {
                    score += WEIGHT_META_WORD;
                }
                if tail.contains(w.as_str()) {
                    score += WEIGHT_TAIL_WORD;
                }
                if tags.contains(w.as_str()) {
                    score += WEIGHT_LIGHT_WORD;
                }
                if people.contains(w.as_str()) {
                    score += WEIGHT_LIGHT_WORD;
                }
            }
            if score > 0 {
                let age = now - m.last_inked;
                score += if age <= RECENT_4H_MS {
                    BONUS_RECENT_4H
                } else if age <= RECENT_1D_MS {
                    BONUS_RECENT_1D
                } else if age <= RECENT_7D_MS {
                    BONUS_RECENT_7D
                } else {
                    0
                };
            }
            (score > 0).then_some((m, score))
        })
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.last_inked.cmp(&a.0.last_inked)));
    hits.into_iter().map(|(m, _)| m).collect()
}

fn stream_dir(book: &Book) -> PathBuf {
    book.dir.join("stream")
}

fn flush_stream(book: &mut Book) -> Result<()> {
    if book.pending.is_empty() {
        return Ok(());
    }
    let seg = book.next_seg;
    book.next_seg += 1;
    write_stream_segment(book, seg, &book.pending)?;
    book.pending.clear();
    Ok(())
}

fn read_stream_on_disk(dir: &Path) -> Result<Vec<StreamRow>> {
    let mut all = Vec::new();
    let stream_dir = dir.join("stream");
    if let Ok(entries) = std::fs::read_dir(&stream_dir) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                all.extend(read_stream_segment(&path)?);
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

fn read_stream_max_seg(dir: &Path) -> Result<u64> {
    let stream_dir = dir.join("stream");
    if !stream_dir.exists() {
        return Ok(0);
    }
    let mut max = 0u64;
    for entry in std::fs::read_dir(&stream_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".parquet")
            && let Ok(n) = stem.parse::<u64>()
        {
            max = max.max(n);
        }
    }
    Ok(max)
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
    ];
    Schema::new(fields).into()
}

fn write_index(book: &Book) -> Result<()> {
    let opened = Int64Array::from(book.index.iter().map(|m| m.opened_at).collect::<Vec<_>>());
    let last_inked = Int64Array::from(book.index.iter().map(|m| m.last_inked).collect::<Vec<_>>());
    let topic = StringArray::from(
        book.index
            .iter()
            .map(|m| m.topic.as_str())
            .collect::<Vec<_>>(),
    );
    let tags = StringArray::from(
        book.index
            .iter()
            .map(|m| join(&m.tags))
            .collect::<Vec<String>>(),
    );
    let people = StringArray::from(
        book.index
            .iter()
            .map(|m| join(&m.people))
            .collect::<Vec<String>>(),
    );
    let facts = StringArray::from(
        book.index
            .iter()
            .map(|m| join(&m.facts))
            .collect::<Vec<String>>(),
    );
    let plans = StringArray::from(
        book.index
            .iter()
            .map(|m| join(&m.plans))
            .collect::<Vec<String>>(),
    );
    let open = StringArray::from(
        book.index
            .iter()
            .map(|m| join(&m.open))
            .collect::<Vec<String>>(),
    );
    let spans = StringArray::from(
        book.index
            .iter()
            .map(|m| join_spans(&m.spans))
            .collect::<Vec<String>>(),
    );
    let tokens = Int64Array::from(book.index.iter().map(|m| m.life_tokens).collect::<Vec<_>>());

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

    let file = std::fs::File::create(book.dir.join("index.parquet"))?;
    let writer = ArrowWriter::try_new(file, index_schema(), None)?;
    let mut writer = writer;
    writer.write(&batch)?;
    let _ = writer.close()?;
    Ok(())
}

fn read_index(path: &Path) -> Result<Vec<CompartmentMeta>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut metas = Vec::new();
    for batch in reader {
        let batch = batch?;
        let opened = batch.column_by_name("opened_at").unwrap();
        let last_inked = batch
            .column_by_name("last_inked")
            .or_else(|| batch.column_by_name("updated_at"))
            .unwrap();
        let topic = batch.column_by_name("topic").unwrap();
        let tags = batch.column_by_name("tags").unwrap();
        let people = batch.column_by_name("people").unwrap();
        let facts = batch.column_by_name("facts").unwrap();
        let plans = batch.column_by_name("plans").unwrap();
        let open = batch.column_by_name("open").unwrap();
        let spans = batch.column_by_name("spans").unwrap();
        let tokens = batch.column_by_name("life_tokens").unwrap();
        for i in 0..batch.num_rows() {
            metas.push(CompartmentMeta {
                opened_at: as_i64(opened, i),
                last_inked: as_i64(last_inked, i),
                topic: as_str(topic, i),
                tags: split(&as_str(tags, i)),
                people: split(&as_str(people, i)),
                facts: split(&as_str(facts, i)),
                plans: split(&as_str(plans, i)),
                open: split(&as_str(open, i)),
                spans: split_spans(&as_str(spans, i)),
                life_tokens: as_i64(tokens, i),
                tail: String::new(),
            });
        }
    }
    Ok(metas)
}

fn write_stream_segment(book: &Book, seg: u64, rows: &[StreamRow]) -> Result<()> {
    let dir = stream_dir(book);
    std::fs::create_dir_all(&dir)?;
    let ids = Int64Array::from(rows.iter().map(|r| r.row_id as i64).collect::<Vec<_>>());
    let role = StringArray::from(rows.iter().map(|r| r.role.as_str()).collect::<Vec<_>>());
    let content = StringArray::from(rows.iter().map(|r| r.content.as_str()).collect::<Vec<_>>());
    let ts = Int64Array::from(rows.iter().map(|r| r.ts).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        stream_schema(),
        vec![
            std::sync::Arc::new(ids),
            std::sync::Arc::new(role),
            std::sync::Arc::new(content),
            std::sync::Arc::new(ts),
        ],
    )?;

    let file = std::fs::File::create(dir.join(format!("{seg:04}.parquet")))?;
    let writer = ArrowWriter::try_new(file, stream_schema(), None)?;
    let mut writer = writer;
    writer.write(&batch)?;
    let _ = writer.close()?;
    Ok(())
}

fn read_stream_segment(path: &Path) -> Result<Vec<StreamRow>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let ids = batch.column_by_name("row_id").unwrap();
        let role = batch.column_by_name("role").unwrap();
        let content = batch.column_by_name("content").unwrap();
        let ts = batch.column_by_name("ts").unwrap();
        for i in 0..batch.num_rows() {
            rows.push(StreamRow {
                row_id: as_i64(ids, i) as u64,
                role: as_str(role, i),
                content: as_str(content, i),
                ts: as_i64(ts, i),
            });
        }
    }
    Ok(rows)
}

fn as_i64(col: &arrow_array::ArrayRef, i: usize) -> i64 {
    col.as_any().downcast_ref::<Int64Array>().unwrap().value(i)
}

fn as_str(col: &arrow_array::ArrayRef, i: usize) -> String {
    col.as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
        .value(i)
        .to_string()
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    fn deem_advances_watermark_and_skips_empty() {
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
        assert_eq!(book.watermark, 0);

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
        assert_eq!(book.watermark, 1);
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
        let mut e = open_engine(&dir).unwrap();

        for _ in 0..4 {
            e.record_turn("user", "a");
        }
        assert_eq!(e.take_prod(), None);

        for _ in 0..40 {
            e.record_turn("user", "this is a long enough sentence to count as weight");
        }
        assert!(e.take_prod().is_some());

        let mut e2 = open_engine(&dir).unwrap();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.note_lookup();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        assert_eq!(e2.take_prod(), None);

        let mut e3 = open_engine(&dir).unwrap();
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
        let mut e = open_engine(&dir).unwrap();

        e.record_turn("user", "remember our lisbon plans");
        e.record_turn("assistant", "may is lovely");

        let book2 = open_book(&dir).unwrap();
        assert_eq!(book2.next_row, 2);
        let rows = read_stream(&book2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content, "remember our lisbon plans");
        assert_eq!(rows[1].content, "may is lovely");

        let mut e2 = open_engine(&dir).unwrap();
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
}
