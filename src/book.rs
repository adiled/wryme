// The book: wryme's memory, kept as a columnar Parquet store so the
// engine can navigate and load threads at scale — the "EPUB for engine
// readership". Grandma never sees it; she just has an AI that remembers.
//
// The stream is the whole truth. Every turn, ever, is written here
// continuously by the engine — user and assistant alike — as append-only
// segments. Nothing is partitioned into compartments; the conversation
// is one continuous thread.
//
// A compartment is NOT a container of turns. It is a distilled bookmark
// that POINTS INTO the stream via spans (start_row..end_row). The same
// stretch of conversation can be deemed into several compartments —
// attribution overlaps and interleaves freely. A thread through its
// lifetime can be deemed into multiple compartments.
//
// Layout:
//   book/stream/<NNNN>.parquet   the content. Append-only message rows,
//                                one continuous record of every turn.
//   book/index.parquet           the navigation. One row per compartment:
//                                its current distilled state (topic, tags,
//                                people, facts, plans, open) plus its
//                                spans into the stream + life_tokens.
//                                A columnar scan never touches content.
//   book/life_summary.txt        prose memory of everything quiet.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Result;
use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;

/// The current distilled state of a compartment — what gets promoted to
/// the conversation's preamble when the thread is re-established. Lists
/// are comma-joined on disk (plain Utf8 columns).
#[derive(Debug, Clone, Default)]
pub struct Bookmark {
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    /// Open threads — what is still in motion. This is the "where we left
    /// off" so the AI can say "we were comparing flight prices".
    pub open: Vec<String>,
}

/// One compartment's current index row. A compartment has NO identity —
/// it is not a container you create and track. It IS a page: a distilled
/// bookmark plus its spans into the stream. It is addressed by what it is
/// (its `topic`), never a number.
#[derive(Debug, Clone)]
pub struct CompartmentMeta {
    pub opened_at: i64,
    pub updated_at: i64,
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
    /// Attributed stretches of the stream, as (start_row..end_row) spans.
    /// The compartment points INTO the stream; it owns no rows itself.
    pub spans: Vec<(u64, u64)>,
    /// Weight of the compartment: sum of content bytes of its spans.
    pub life_tokens: i64,
}

/// One turn in the continuous stream. `content` is the rendered text the
/// model reads.
#[derive(Debug, Clone)]
pub struct StreamRow {
    pub row_id: u64,
    pub role: String,
    pub content: String,
    pub ts: i64,
}

/// One message, as read out of a compartment's spans (for rendering).
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub ts: i64,
}

/// The book. Index rows live in memory (small); the stream stays on disk
/// and is loaded per read. `watermark` is the first row NOT yet attributed
/// to any compartment — the unattributed frontier the engine watches.
pub struct Book {
    dir: PathBuf,
    next_row: u64,
    /// The next stream segment number — a monotonic counter kept in
    /// memory so flushing a turn is O(1), not a directory scan.
    next_seg: u64,
    watermark: u64,
    /// Content bytes of the unattributed rows [watermark, next_row) —
    /// the engine's weight signal for the prod.
    unattr_tokens: i64,
    unattr_turns: i64,
    index: Vec<CompartmentMeta>,
    /// Rows not yet flushed to a stream segment.
    pending: Vec<StreamRow>,
    life_summary: String,
}

/// The engine: the live, in-memory face of the book that the conversation
/// talks to. Holds the book plus which compartments are currently
/// promoted to the conversation's preamble, and the book-writing prod
/// state. Shared across turns as an `Arc<Mutex<Engine>>` so the streaming
/// protocol and the background delivery can both reach it.
///
/// The engine is NOT an agent. It writes the stream continuously and
/// prods the agent to deem compartments; establishing and reminiscing
/// remain the agent's own deliberation.
pub struct Engine {
    pub book: Book,
    /// Pages currently promoted to the preamble. Their rendered
    /// bookmarks are prepended as system messages to every request.
    pub established: Vec<String>,
    /// Set when an unattributed thread is weightful; delivered once into
    /// the next request as a quiet system reminder.
    pending_prod: bool,
    /// The stream row when a prod was last delivered — a cooldown so it
    /// nudges, waits a few turns, and nudges again only if still unfiled
    /// (it never nags every turn).
    prod_delivered_row: u64,
    /// The stream row when the last lookup (find/open) happened — used
    /// for the guard: no prod within a few turns of a lookup.
    last_lookup_row: u64,
}

/// Weight thresholds for the book-writing prod. A thread must accumulate
/// enough turns AND tokens, with no recent lookup, before the engine
/// nudges the agent to deem it.
const WEIGHT_TURNS: i64 = 5;
const WEIGHT_BYTES: i64 = 1600;
/// Turns of distance from an establishing-related lookup that shield the
/// thread from a prod (the agent is actively reminiscing over it).
const LOOKUP_GAP: u64 = 5;

/// Open the engine (book + empty preamble) in `dir`.
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
    /// The preamble: rendered bookmark prose for every established page,
    /// in order. Prepended as system messages by the caller.
    pub fn preamble(&self) -> Vec<String> {
        self.established
            .iter()
            .filter_map(|topic| compartment(&self.book, topic))
            .map(render_bookmark)
            .collect()
    }

    /// The book-writing prod: deliver it once into the next request, then
    /// cool down. The engine slips the agent a quiet system reminder when
    /// an unattributed thread is weightful and unshielded; it nudges, waits
    /// a few turns, and nudges again only if the thread is still unfiled.
    /// Never shown to grandma.
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

    /// Write one turn into the continuous stream, then re-check the prod.
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
        // Flush to disk now, so every turn is durable even if the agent
        // never deems and the app exits. The stream truly writes
        // continuously; deeming later only points rows at pages.
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

    /// Mark that a lookup (find/open) happened at the current stream
    /// position — the shield that suppresses a prod while the agent is
    /// actively reminiscing over a thread.
    pub fn note_lookup(&mut self) {
        self.last_lookup_row = self.book.next_row;
    }

    /// Promote a page to the preamble and return its rendered bookmark
    /// plus full thread text (so the model can just know). This is the
    /// ceremony of "remember when" — the agent's own deliberation.
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

    /// Attribute the unattributed span [watermark, next_row) to the page
    /// named by the bookmark's topic (birthing it if absent) and refresh
    /// its distilled bookmark. This is quiet book-keeping — it does NOT
    /// establish (no ceremony, no preamble). Clears the prod.
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

    /// Drop a page from the preamble (it is never closed — it just stops
    /// riding along in the conversation).
    pub fn dismiss(&mut self, topic: &str) {
        self.established.retain(|t| t != topic);
    }
}

/// Open (or create) the book in `dir`.
pub fn open_book(dir: &Path) -> Result<Book> {
    std::fs::create_dir_all(dir)?;
    let index_path = dir.join("index.parquet");
    let mut index = if index_path.exists() {
        read_index(&index_path)?
    } else {
        vec![]
    };
    // Drop ghost pages — rows with no topic are leftovers from the old
    // "new without deem" registry; a page is only a page if it is about
    // something.
    index.retain(|m| !m.topic.trim().is_empty());
    let next_row = read_stream_max_row(dir)?;
    let next_seg = read_stream_max_seg(dir)? + 1;

    let summary_path = dir.join("life_summary.txt");
    let life_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();

    Ok(Book {
        dir: dir.to_path_buf(),
        next_row,
        next_seg,
        // On a fresh open the frontier is the whole stream (everything is
        // unattributed); on a continued open the frontier is restored to
        // the end so we never re-deem already-attributed rows.
        watermark: next_row,
        unattr_tokens: 0,
        unattr_turns: 0,
        index,
        pending: vec![],
        life_summary,
    })
}

/// Attribute the unattributed span to a page, addressed by its topic.
/// If no page with that topic exists it is BORN here — book-writing is
/// the product, not the precondition. If several pages share the topic
/// they merge into one (a page's identity is its name + spans). Records
/// the span, refreshes the distilled bookmark, adds its weight, flushes
/// the rows to the stream, and advances the watermark. The page owns no
/// rows — it only points into the stream.
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
            // A duplicate same-name page: fold its spans into the first
            // and drop it, so the invariant "one page per topic" holds.
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
                updated_at: now,
                topic: topic.to_string(),
                tags: vec![],
                people: vec![],
                facts: vec![],
                plans: vec![],
                open: vec![],
                spans: vec![],
                life_tokens: 0,
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
    meta.updated_at = now;
    flush_stream(book)?;
    book.watermark = end;
    book.unattr_tokens = 0;
    book.unattr_turns = 0;
    write_index(book)?;
    Ok(format!(
        "deemed rows {start}..{end} into \"{topic}\"; bookmark refreshed"
    ))
}

/// Refresh a page's distilled state (and updated_at) without deeming new
/// rows. Called when a thread is re-established and the AI re-distills
/// what matters. Addressed by topic.
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
    meta.updated_at = now_ms();
    write_index(book)?;
    Ok(())
}

/// All index rows — the navigation layer, small and always in context.
pub fn index_entries(book: &Book) -> &[CompartmentMeta] {
    &book.index
}

/// Look up one page's current index row, by its topic.
pub fn compartment<'a>(book: &'a Book, topic: &'a str) -> Option<&'a CompartmentMeta> {
    book.index.iter().find(|m| eq_topic(&m.topic, topic))
}

/// Read a page's full thread — every row its spans point into the
/// stream, concatenated in order.
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

/// The preamble: render a page's distilled state to the prose the model
/// reads at the top of the conversation. One page, one system message.
pub fn render_bookmark(meta: &CompartmentMeta) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", meta.topic));
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

/// Render a compartment's full thread text (for reading on demand).
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

/// Deterministic retrieval: match grandma's words against the index
/// columns (topic / tags / people / facts / plans / open). No embeddings —
/// just keyword overlap over the metadata, which a columnar scan makes
/// cheap at scale.
pub fn match_compartments<'a>(book: &'a Book, query: &str) -> Vec<&'a CompartmentMeta> {
    let q = query.to_lowercase();
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(&CompartmentMeta, usize)> = book
        .index
        .iter()
        .filter_map(|m| {
            let hay = format!(
                "{} {} {} {} {} {}",
                m.topic.to_lowercase(),
                m.tags.join(" ").to_lowercase(),
                m.people.join(" ").to_lowercase(),
                m.facts.join(" ").to_lowercase(),
                m.plans.join(" ").to_lowercase(),
                m.open.join(" ").to_lowercase(),
            );
            let score = words
                .iter()
                .filter(|w| hay.contains(w.as_str()))
                .count();
            (score > 0).then_some((m, score))
        })
        .collect();
    hits.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
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

fn read_stream(book: &Book) -> Result<Vec<StreamRow>> {
    let mut all = Vec::new();
    let dir = stream_dir(book);
    if dir.exists() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                all.extend(read_stream_segment(&path)?);
            }
        }
    }
    all.extend(book.pending.iter().cloned());
    all.sort_by_key(|r| r.row_id);
    Ok(all)
}

/// The highest row id already on disk — so the frontier resumes correctly
/// across restarts.
fn read_stream_max_row(dir: &Path) -> Result<u64> {
    let stream_dir = dir.join("stream");
    if !stream_dir.exists() {
        return Ok(0);
    }
    let mut max = 0u64;
    for entry in std::fs::read_dir(&stream_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "parquet").unwrap_or(false) {
            let seg = read_stream_segment(&path)?;
            for r in &seg {
                max = max.max(r.row_id + 1);
            }
        }
    }
    Ok(max)
}

/// The highest stream segment number on disk — so the flush counter
/// resumes correctly across restarts.
fn read_stream_max_seg(dir: &Path) -> Result<u64> {
    let stream_dir = dir.join("stream");
    if !stream_dir.exists() {
        return Ok(0);
    }
    let mut max = 0u64;
    for entry in std::fs::read_dir(&stream_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".parquet") {
            if let Ok(n) = stem.parse::<u64>() {
                max = max.max(n);
            }
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
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    *spans = merged;
}

// ---- parquet read/write ----------------------------------------------

/// The navigation schema — distilled state + spans, no content, no
/// compartment id. A page is addressed by its topic.
fn index_schema() -> SchemaRef {
    let fields = vec![
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
    ];
    Schema::new(fields).into()
}

/// The stream schema — one row per turn, ever.
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
    let opened = Int64Array::from(
        book.index.iter().map(|m| m.opened_at).collect::<Vec<_>>(),
    );
    let updated = Int64Array::from(
        book.index.iter().map(|m| m.updated_at).collect::<Vec<_>>(),
    );
    let topic = StringArray::from(
        book.index.iter().map(|m| m.topic.as_str()).collect::<Vec<_>>(),
    );
    let tags = StringArray::from(
        book.index.iter().map(|m| join(&m.tags)).collect::<Vec<String>>(),
    );
    let people = StringArray::from(
        book.index.iter().map(|m| join(&m.people)).collect::<Vec<String>>(),
    );
    let facts = StringArray::from(
        book.index.iter().map(|m| join(&m.facts)).collect::<Vec<String>>(),
    );
    let plans = StringArray::from(
        book.index.iter().map(|m| join(&m.plans)).collect::<Vec<String>>(),
    );
    let open = StringArray::from(
        book.index.iter().map(|m| join(&m.open)).collect::<Vec<String>>(),
    );
    let spans = StringArray::from(
        book.index
            .iter()
            .map(|m| join_spans(&m.spans))
            .collect::<Vec<String>>(),
    );
    let tokens = Int64Array::from(
        book.index.iter().map(|m| m.life_tokens).collect::<Vec<_>>(),
    );

    let batch = RecordBatch::try_new(
        index_schema(),
        vec![
            std::sync::Arc::new(opened),
            std::sync::Arc::new(updated),
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
        let updated = batch.column_by_name("updated_at").unwrap();
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
                updated_at: as_i64(updated, i),
                topic: as_str(topic, i),
                tags: split(&as_str(tags, i)),
                people: split(&as_str(people, i)),
                facts: split(&as_str(facts, i)),
                plans: split(&as_str(plans, i)),
                open: split(&as_str(open, i)),
                spans: split_spans(&as_str(spans, i)),
                life_tokens: as_i64(tokens, i),
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
    col
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(i)
}

fn as_str(col: &arrow_array::ArrayRef, i: usize) -> String {
    col
        .as_any()
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
        flush_stream(&mut book).unwrap(); // empty — no-op

        // One continuous record, in order, regardless of compartment.
        let rows = read_stream(&book).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content, "hello");
        assert_eq!(rows[1].content, "hi there");

        // Restart: the frontier resumes at the end of what is on disk.
        let book2 = open_book(&dir).unwrap();
        assert_eq!(book2.next_row, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deem_attributes_spans_and_multiple_pages_interleave() {
        let dir = tmpdir("deem");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();

        // Turns 0..3 about the garden, 3..6 about the neighbours.
        for c in ["soil", "roses", "shade"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(&mut book, &Bookmark { topic: "garden".into(), ..Default::default() }).unwrap();

        for c in ["dog", "fence", "noise"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(&mut book, &Bookmark { topic: "neighbours".into(), ..Default::default() }).unwrap();

        // Back to the garden later: a second, non-contiguous span.
        for c in ["compost", "mulch"] {
            book.pending.push(msg("user", c, book.next_row));
            book.next_row += 1;
            book.unattr_turns += 1;
            book.unattr_tokens += c.len() as i64;
        }
        deem_span(&mut book, &Bookmark { topic: "garden".into(), ..Default::default() }).unwrap();

        let g = compartment(&book, "garden").unwrap();
        let n = compartment(&book, "neighbours").unwrap();
        assert_eq!(g.spans, vec![(0, 3), (6, 8)]); // interleaved, not merged
        assert_eq!(n.spans, vec![(3, 6)]);

        let g_thread = read_compartment(&book, "garden").unwrap().unwrap();
        assert_eq!(g_thread.len(), 5); // rows 0..3 + 6..8
        assert_eq!(g_thread[0].content, "soil");
        assert_eq!(g_thread[4].content, "mulch");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deem_advances_watermark_and_skips_empty() {
        let dir = tmpdir("watermark");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();

        assert!(deem_span(&mut book, &Bookmark { topic: "t".into(), ..Default::default() }).unwrap().contains("no new turns"));
        assert_eq!(book.watermark, 0);

        book.pending.push(msg("user", "one", book.next_row));
        book.next_row += 1;
        book.unattr_turns += 1;
        book.unattr_tokens += 3;
        deem_span(&mut book, &Bookmark { topic: "t".into(), ..Default::default() }).unwrap();
        assert_eq!(book.watermark, 1);
        assert_eq!(book.unattr_tokens, 0);
        assert_eq!(compartment(&book, "t").unwrap().spans, vec![(0, 1)]);

        // Nothing new to deem now.
        assert!(deem_span(&mut book, &Bookmark { topic: "t".into(), ..Default::default() }).unwrap().contains("no new turns"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_prod_fires_only_when_weightful_and_unshielded() {
        let dir = tmpdir("prod");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir).unwrap();

        // A few short turns: not weightful yet.
        for _ in 0..4 {
            e.record_turn("user", "a");
        }
        assert_eq!(e.take_prod(), None);

        // Enough turns + tokens, no lookup shield → prod.
        for _ in 0..40 {
            e.record_turn("user", "this is a long enough sentence to count as weight");
        }
        assert!(e.take_prod().is_some());

        // A lookup shields it.
        let mut e2 = open_engine(&dir).unwrap();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.note_lookup();
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        e2.record_turn("user", "this is a long enough sentence to count as weight");
        // still within the lookup gap
        assert_eq!(e2.take_prod(), None);

        // Deeming clears the prod.
        let mut e3 = open_engine(&dir).unwrap();
        for _ in 0..40 {
            e3.record_turn("user", "this is a long enough sentence to count as weight");
        }
        e3.deem(&Bookmark { topic: "x".into(), ..Default::default() }).unwrap();
        assert_eq!(e3.take_prod(), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_flushes_every_turn_so_turns_survive_without_deem() {
        let dir = tmpdir("durable");
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = open_engine(&dir).unwrap();

        // Two turns, never deemed — the old gap lost them on exit.
        e.record_turn("user", "remember our lisbon plans");
        e.record_turn("assistant", "may is lovely");

        // Simulate an exit: reopen from disk, no deem ever happened.
        let book2 = open_book(&dir).unwrap();
        assert_eq!(book2.next_row, 2);
        let rows = read_stream(&book2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content, "remember our lisbon plans");
        assert_eq!(rows[1].content, "may is lovely");

        // The segment counter resumes, so a later flush appends, never
        // overwrites the existing segments.
        let mut e2 = open_engine(&dir).unwrap();
        e2.record_turn("user", "third turn");
        let book3 = open_book(&dir).unwrap();
        let rows = read_stream(&book3).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].content, "third turn");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
