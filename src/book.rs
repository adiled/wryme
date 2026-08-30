// The book: wryme's memory, kept as a columnar Parquet store so the
// engine can navigate and load threads at scale — the "EPUB for engine
// readership". Grandma never sees it; she just has an AI that remembers.
//
// A compartment is an open-ended thread. It is NEVER closed: it
#![allow(dead_code)] // life_summary / index_entries await the engine's full use
// accumulates over time, across windows and reboots. Each time the thread
// is continued, another segment file is appended; its index row always
// holds the CURRENT distilled state, never a "closed" snapshot.
//
// Layout, mirroring EPUB's toc-vs-chapters split:
//   book/index.parquet          the navigation layer. One row per
//                               compartment: its distilled state (topic,
//                               tags, people, facts, plans, open threads)
//                               plus opened_at / updated_at / life_tokens.
//                               This is what the engine scans; a columnar
//                               scan never touches thread content.
//   book/compartments/<id>/     the content layer. Segment files
//                               <seg>.parquet, message rows for one chunk
//                               of a thread, appended as it continues.
//                               Read on demand; addressable by id.
//   book/life_summary.txt       prose memory of everything quiet, kept by
//                               the engine. Always in context as text.

use std::path::{Path, PathBuf};

use anyhow::Result;
use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;

use crate::api::ApiMessage;

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

/// One compartment's current index row.
#[derive(Debug, Clone)]
pub struct CompartmentMeta {
    pub id: u64,
    pub opened_at: i64,
    pub updated_at: i64,
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
    pub life_tokens: i64,
}

/// One message in a compartment segment. `content` is the rendered text
/// the model reads.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub ts: i64,
}

/// The book. Index rows live in memory (small); thread content stays on
/// disk and is loaded per compartment.
pub struct Book {
    dir: PathBuf,
    next_id: u64,
    index: Vec<CompartmentMeta>,
    life_summary: String,
}

/// The engine: the live, in-memory face of the book that the conversation
/// talks to. Holds the book plus which compartments are currently
/// promoted to the conversation's preamble. Shared across turns as an
/// `Arc<Mutex<Engine>>` so the streaming protocol and the background
/// delivery can both reach it.
pub struct Engine {
    pub book: Book,
    /// Compartments currently promoted to the preamble. Their rendered
    /// bookmarks are prepended as system messages to every request.
    pub established: Vec<u64>,
}

/// Open the engine (book + empty preamble) in `dir`.
pub fn open_engine(dir: &Path) -> Result<Engine> {
    Ok(Engine {
        book: open_book(dir)?,
        established: vec![],
    })
}

impl Engine {
    /// The preamble: rendered bookmark prose for every established
    /// compartment, in order. Prepended as system messages by the caller.
    pub fn preamble(&self) -> Vec<String> {
        self.established
            .iter()
            .filter_map(|id| compartment(&self.book, *id))
            .map(render_bookmark)
            .collect()
    }

    fn establish(&mut self, id: u64) {
        if !self.established.contains(&id) {
            self.established.push(id);
        }
    }

    /// Drop a compartment from the preamble (it is never closed — it
    /// just stops riding along in the conversation).
    pub fn dismiss(&mut self, id: u64) {
        self.established.retain(|&e| e != id);
    }

    /// Promote a compartment to the preamble and return its rendered
    /// bookmark plus full thread text (so the model can just know).
    pub fn open(&mut self, id: u64) -> Result<Option<String>> {
        let Some(meta) = compartment(&self.book, id).cloned() else {
            return Ok(None);
        };
        self.establish(id);
        let mut out = render_bookmark(&meta);
        if let Some(thread) = read_compartment(&self.book, id)? {
            out.push_str("\n\n--- thread ---\n\n");
            out.push_str(&render_compartment(&thread));
        }
        Ok(Some(out))
    }

    /// Append this sitting's new turns to a compartment and refresh its
    /// distilled bookmark. Turns already recorded in the compartment are
    /// skipped (continuations don't duplicate). Promotes it to the
    /// preamble. Returns a short confirmation.
    pub fn append(
        &mut self,
        id: u64,
        bookmark: &Bookmark,
        session: &[ApiMessage],
    ) -> Result<String> {
        let existing = read_compartment(&self.book, id)?;
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        if let Some(msgs) = &existing {
            for m in msgs {
                seen.insert((m.role.clone(), m.content.clone()));
            }
        }
        let mut fresh: Vec<Message> = Vec::new();
        for m in session {
            if m.role == "system" {
                continue;
            }
            let key = (m.role.clone(), m.content.clone());
            if seen.insert(key.clone()) {
                fresh.push(Message {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    ts: now_ms(),
                });
            }
        }
        let segs = append_segment(&mut self.book, id, &fresh)?;
        update_bookmark(&mut self.book, id, bookmark)?;
        self.establish(id);
        Ok(format!(
            "appended {} new message(s) to compartment #{id} (segment {segs}); bookmark refreshed",
            fresh.len()
        ))
    }
}

/// The columnar index schema — metadata only, no content.
fn index_schema() -> SchemaRef {
    let fields = vec![
        Field::new("compartment_id", DataType::Int64, false),
        Field::new("opened_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("topic", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, true),
        Field::new("people", DataType::Utf8, true),
        Field::new("facts", DataType::Utf8, true),
        Field::new("plans", DataType::Utf8, true),
        Field::new("open", DataType::Utf8, true),
        Field::new("life_tokens", DataType::Int64, false),
    ];
    Schema::new(fields).into()
}

/// The content schema — message rows for one segment of a compartment.
fn segment_schema() -> SchemaRef {
    let fields = vec![
        Field::new("compartment_id", DataType::Int64, false),
        Field::new("seq", DataType::Int64, false),
        Field::new("role", DataType::Utf8, true),
        Field::new("content", DataType::Utf8, true),
        Field::new("ts", DataType::Int64, false),
    ];
    Schema::new(fields).into()
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Open (or create) the book in `dir`.
pub fn open_book(dir: &Path) -> Result<Book> {
    std::fs::create_dir_all(dir)?;
    let index_path = dir.join("index.parquet");
    let index = if index_path.exists() {
        read_index(&index_path)?
    } else {
        vec![]
    };
    let next_id = index.iter().map(|m| m.id + 1).max().unwrap_or(1);

    let summary_path = dir.join("life_summary.txt");
    let life_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();

    Ok(Book {
        dir: dir.to_path_buf(),
        next_id,
        index,
        life_summary,
    })
}

/// Start a brand-new compartment and return its id. The thread is open
/// ended: it will accumulate segments forever. The distilled state starts
/// empty and is set by `update_bookmark` as the thread develops.
pub fn new_compartment(book: &mut Book) -> Result<u64> {
    let id = book.next_id;
    book.next_id += 1;
    let now = now_ms();
    book.index.push(CompartmentMeta {
        id,
        opened_at: now,
        updated_at: now,
        topic: String::new(),
        tags: vec![],
        people: vec![],
        facts: vec![],
        plans: vec![],
        open: vec![],
        life_tokens: 0,
    });
    write_index(book)?;
    Ok(id)
}

/// Append one chunk of a thread as a new segment file for a compartment.
/// Returns the number of segments the compartment now has.
pub fn append_segment(book: &mut Book, id: u64, messages: &[Message]) -> Result<u64> {
    let seg = next_segment(book, id)?;
    let Some(meta) = book.index.iter_mut().find(|m| m.id == id) else {
        return Ok(seg); // unknown compartment — nothing appended
    };
    meta.life_tokens += messages.iter().map(|m| m.content.len() as i64).sum::<i64>();
    meta.updated_at = now_ms();
    write_segment(book, id, seg, messages)?;
    write_index(book)?;
    Ok(seg + 1)
}

/// Refresh a compartment's distilled state (and updated_at). Called as the
/// thread develops — the AI writes its own bookmark, the engine stores it.
pub fn update_bookmark(book: &mut Book, id: u64, bookmark: &Bookmark) -> Result<()> {
    let Some(meta) = book.index.iter_mut().find(|m| m.id == id) else {
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

/// Look up one compartment's current index row.
pub fn compartment(book: &Book, id: u64) -> Option<&CompartmentMeta> {
    book.index.iter().find(|m| m.id == id)
}

/// Read a compartment's full thread — all segments, concatenated in order.
pub fn read_compartment(book: &Book, id: u64) -> Result<Option<Vec<Message>>> {
    let dir = compartment_dir(book, id);
    if !dir.exists() {
        return Ok(None);
    }
    let mut all = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "parquet").unwrap_or(false) {
            all.extend(read_segment(&path)?);
        }
    }
    all.sort_by_key(|m| m.ts);
    Ok(Some(all))
}

/// The preamble: render a compartment's distilled state to the prose the
/// model reads at the top of the conversation. One compartment, one
/// system message.
pub fn render_bookmark(meta: &CompartmentMeta) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Compartment {}: {}\n", meta.id, meta.topic));
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

fn compartment_dir(book: &Book, id: u64) -> PathBuf {
    book.dir.join("compartments").join(format!("{id:04}"))
}

fn next_segment(book: &Book, id: u64) -> Result<u64> {
    let dir = compartment_dir(book, id);
    if !dir.exists() {
        return Ok(0);
    }
    let mut max = 0u64;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".parquet") {
            if let Ok(n) = stem.parse::<u64>() {
                max = max.max(n);
            }
        }
    }
    Ok(max + 1)
}

// ---- parquet read/write ----------------------------------------------

fn write_index(book: &Book) -> Result<()> {
    let ids = Int64Array::from(
        book.index.iter().map(|m| m.id as i64).collect::<Vec<_>>(),
    );
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
    let tokens = Int64Array::from(
        book.index.iter().map(|m| m.life_tokens).collect::<Vec<_>>(),
    );

    let batch = RecordBatch::try_new(
        index_schema(),
        vec![
            std::sync::Arc::new(ids),
            std::sync::Arc::new(opened),
            std::sync::Arc::new(updated),
            std::sync::Arc::new(topic),
            std::sync::Arc::new(tags),
            std::sync::Arc::new(people),
            std::sync::Arc::new(facts),
            std::sync::Arc::new(plans),
            std::sync::Arc::new(open),
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
        let ids = batch.column_by_name("compartment_id").unwrap();
        let opened = batch.column_by_name("opened_at").unwrap();
        let updated = batch.column_by_name("updated_at").unwrap();
        let topic = batch.column_by_name("topic").unwrap();
        let tags = batch.column_by_name("tags").unwrap();
        let people = batch.column_by_name("people").unwrap();
        let facts = batch.column_by_name("facts").unwrap();
        let plans = batch.column_by_name("plans").unwrap();
        let open = batch.column_by_name("open").unwrap();
        let tokens = batch.column_by_name("life_tokens").unwrap();
        for i in 0..batch.num_rows() {
            metas.push(CompartmentMeta {
                id: as_i64(ids, i) as u64,
                opened_at: as_i64(opened, i),
                updated_at: as_i64(updated, i),
                topic: as_str(topic, i),
                tags: split(&as_str(tags, i)),
                people: split(&as_str(people, i)),
                facts: split(&as_str(facts, i)),
                plans: split(&as_str(plans, i)),
                open: split(&as_str(open, i)),
                life_tokens: as_i64(tokens, i),
            });
        }
    }
    Ok(metas)
}

fn write_segment(book: &Book, id: u64, seg: u64, messages: &[Message]) -> Result<()> {
    let dir = compartment_dir(book, id);
    std::fs::create_dir_all(&dir)?;
    let n = messages.len();
    let ids = Int64Array::from(vec![id as i64; n]);
    let seq = Int64Array::from((0..n as i64).collect::<Vec<_>>());
    let role = StringArray::from(
        messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
    );
    let content = StringArray::from(
        messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
    );
    let ts = Int64Array::from(messages.iter().map(|m| m.ts).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        segment_schema(),
        vec![
            std::sync::Arc::new(ids),
            std::sync::Arc::new(seq),
            std::sync::Arc::new(role),
            std::sync::Arc::new(content),
            std::sync::Arc::new(ts),
        ],
    )?;

    let file = std::fs::File::create(dir.join(format!("{seg:04}.parquet")))?;
    let writer = ArrowWriter::try_new(file, segment_schema(), None)?;
    let mut writer = writer;
    writer.write(&batch)?;
    let _ = writer.close()?;
    Ok(())
}

fn read_segment(path: &Path) -> Result<Vec<Message>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut msgs = Vec::new();
    for batch in reader {
        let batch = batch?;
        let role = batch.column_by_name("role").unwrap();
        let content = batch.column_by_name("content").unwrap();
        let ts = batch.column_by_name("ts").unwrap();
        for i in 0..batch.num_rows() {
            msgs.push(Message {
                role: as_str(role, i),
                content: as_str(content, i),
                ts: as_i64(ts, i),
            });
        }
    }
    Ok(msgs)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wryme_book_{tag}_{}", std::process::id()))
    }

    fn msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: content.to_string(),
            ts: now_ms(),
        }
    }

    #[test]
    fn compartment_appends_segments_open_ended() {
        let dir = tmpdir("segments");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let id = new_compartment(&mut book).unwrap();

        // First sitting: two messages.
        let segs = append_segment(
            &mut book,
            id,
            &[msg("user", "let's plan Lisbon"), msg("assistant", "May is nice")],
        )
        .unwrap();
        assert_eq!(segs, 1);

        // A second sitting, days later, on the same open-ended thread.
        let segs2 = append_segment(
            &mut book,
            id,
            &[msg("user", "flights got cheaper"), msg("assistant", "book them")],
        )
        .unwrap();
        assert_eq!(segs2, 2);

        update_bookmark(
            &mut book,
            id,
            &Bookmark {
                topic: "Lisbon trip".to_string(),
                tags: vec!["travel".to_string(), "lisbon".to_string()],
                people: vec!["grandma".to_string()],
                facts: vec!["flights cheaper".to_string()],
                plans: vec!["book flights".to_string()],
                open: vec!["comparing prices".to_string()],
            },
        )
        .unwrap();

        // Reload from disk and read the whole thread back, in order.
        let book2 = open_book(&dir).unwrap();
        let all = read_compartment(&book2, id).unwrap().unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].content, "let's plan Lisbon");
        assert_eq!(all[2].content, "flights got cheaper");

        let metas = index_entries(&book2);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].topic, "Lisbon trip");
        assert_eq!(metas[0].open, vec!["comparing prices"]);
        assert_eq!(metas[0].life_tokens, 56); // sum of content byte lengths
        // Open ended: no closed_at anywhere.
        assert!(render_bookmark(&metas[0]).contains("Lisbon trip"));
        assert!(render_bookmark(&metas[0]).contains("comparing prices"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_compartments_ranks_by_keyword_overlap() {
        let dir = tmpdir("match");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let a = new_compartment(&mut book).unwrap();
        update_bookmark(
            &mut book,
            a,
            &Bookmark {
                topic: "Lisbon trip".to_string(),
                tags: vec!["travel".to_string(), "lisbon".to_string()],
                facts: vec!["may".to_string()],
                ..Bookmark::default()
            },
        )
        .unwrap();
        let b = new_compartment(&mut book).unwrap();
        update_bookmark(
            &mut book,
            b,
            &Bookmark {
                topic: "Miso the cat".to_string(),
                tags: vec!["cat".to_string(), "vet".to_string()],
                facts: vec!["vet tuesday".to_string()],
                ..Bookmark::default()
            },
        )
        .unwrap();

        let hits = match_compartments(&book, "lisbon may dates");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic, "Lisbon trip");

        let cat = match_compartments(&book, "take miso to the vet");
        assert_eq!(cat[0].topic, "Miso the cat");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_monotonic_after_reopen() {
        let dir = tmpdir("ids");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let id = new_compartment(&mut book).unwrap();
        append_segment(&mut book, id, &[msg("user", "hi")]).unwrap();
        drop(book);
        let mut book2 = open_book(&dir).unwrap();
        let id2 = new_compartment(&mut book2).unwrap();
        assert!(id2 > id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
