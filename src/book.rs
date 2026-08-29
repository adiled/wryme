// The book: wryme's memory, kept as a columnar Parquet store so the
// engine can navigate and load pages at scale — the "EPUB for engine
// readership". Grandma never sees it; she just has an AI that remembers.
//
// Two layers, mirroring EPUB's toc-vs-chapters split:
//   book/index.parquet          the navigation layer. One row per closed
//                               page, holding ONLY the small metadata
//                               columns (topic, tags, people, ...). This
//                               is what the model keeps warm in context,
//                               and a columnar scan of it never touches
//                               page content.
//   book/pages/<id>.parquet     the content layer. Message rows for one
//                               page, addressable by id and loaded on
//                               demand. Each page is one immutable file,
//                               so writing a new page never rewrites the
//                               big data — only the small index.
//
//   book/life_summary.txt       the merged memory of everything old; the
//                               anti-bloat. Old pages fold into this and
//                               their content files are dropped.
//
// Only open_book is live right now. The curation + consultation API
// (new_page, close_page, load_page, match_pages, compact) awaits the
// engine wiring — feeding the index to the model's context and closing
// pages on thread drift — so those pieces are allowed-dead until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use arrow_array::{Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;

/// What the engine keeps from a closed page — the bookmark the model
/// reads back later. Lists are comma-joined on disk (plain Utf8 columns).
#[derive(Debug, Clone, Default)]
pub struct Bookmark {
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
}

/// One closed page's metadata — a single index row.
#[derive(Debug, Clone)]
pub struct PageMeta {
    pub page_id: u64,
    pub opened_at: i64,
    pub closed_at: i64,
    pub topic: String,
    pub tags: Vec<String>,
    pub people: Vec<String>,
    pub facts: Vec<String>,
    pub plans: Vec<String>,
    pub open: Vec<String>,
    pub life_tokens: i64,
}

/// One message in a page. `content` is the rendered text the model reads.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub ts: i64,
}

/// The book. Index rows live in memory (small); page content stays on
/// disk and is loaded per page.
pub struct Book {
    dir: PathBuf,
    next_id: u64,
    index: Vec<PageMeta>,
    open_pages: HashMap<u64, i64>, // page_id -> opened_at (epoch ms)
    life_summary: String,
}

/// The columnar index schema — metadata only, no content.
fn index_schema() -> SchemaRef {
    let fields = vec![
        Field::new("page_id", DataType::Int64, false),
        Field::new("opened_at", DataType::Int64, false),
        Field::new("closed_at", DataType::Int64, false),
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

/// The content schema — message rows for one page.
fn page_schema() -> SchemaRef {
    let fields = vec![
        Field::new("page_id", DataType::Int64, false),
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
    let next_id = index.iter().map(|m| m.page_id + 1).max().unwrap_or(1);

    let summary_path = dir.join("life_summary.txt");
    let life_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();

    Ok(Book {
        dir: dir.to_path_buf(),
        next_id,
        index,
        open_pages: HashMap::new(),
        life_summary,
    })
}

/// Open a fresh page and return its id. Messages accumulate in the live
/// app state until `close_page` files it away.
pub fn new_page(book: &mut Book) -> u64 {
    let id = book.next_id;
    book.next_id += 1;
    book.open_pages.insert(id, now_ms());
    id
}

/// Close a page: write its content file (immutable) and fold its metadata
/// into the index. The live page's messages are the model's reading
/// material, so this is where the engine decides what to bookmark.
pub fn close_page(
    book: &mut Book,
    page_id: u64,
    messages: &[Message],
    bookmark: &Bookmark,
) -> Result<()> {
    let opened_at = book.open_pages.remove(&page_id).unwrap_or_else(now_ms);
    let closed_at = now_ms();
    let life_tokens: i64 = messages
        .iter()
        .map(|m| m.content.len() as i64)
        .sum::<i64>();

    write_page(book, page_id, messages)?;

    let meta = PageMeta {
        page_id,
        opened_at,
        closed_at,
        topic: bookmark.topic.clone(),
        tags: bookmark.tags.clone(),
        people: bookmark.people.clone(),
        facts: bookmark.facts.clone(),
        plans: bookmark.plans.clone(),
        open: bookmark.open.clone(),
        life_tokens,
    };
    book.index.push(meta);
    write_index(book)?;
    Ok(())
}

/// All index rows — the navigation layer, small and always in context.
pub fn index_entries(book: &Book) -> &[PageMeta] {
    &book.index
}

/// Load one page's messages from its content file.
pub fn load_page(book: &Book, page_id: u64) -> Result<Option<Vec<Message>>> {
    let path = page_path(book, page_id);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_page(&path)?))
}

/// Render a page's messages back to the plain text the model reads.
pub fn render_page(messages: &[Message]) -> String {
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

/// Deterministic retrieval: match grandma's words against the small index
/// columns (topic / tags / people). No embeddings — just keyword overlap
/// over the metadata, which a columnar scan makes cheap at scale.
pub fn match_pages<'a>(book: &'a Book, query: &str) -> Vec<&'a PageMeta> {
    let q = query.to_lowercase();
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(&PageMeta, usize)> = book
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

/// Anti-bloat: fold the oldest index entries into the life summary and
/// drop their content files, keeping only the newest `keep` pages detailed.
pub fn compact(book: &mut Book, keep: usize) -> Result<()> {
    if book.index.len() <= keep {
        return Ok(());
    }
    let drop = book.index.len() - keep;
    let mut folded = Vec::new();
    for m in &book.index[..drop] {
        let line = format!(
            "{} (page {}): {}. People: {}. Facts: {}. Plans: {}. Open: {}",
            m.topic,
            m.page_id,
            m.tags.join(", "),
            m.people.join(", "),
            m.facts.join(", "),
            m.plans.join(", "),
            m.open.join(", ")
        );
        folded.push(line);
        let _ = std::fs::remove_file(page_path(book, m.page_id));
    }
    let new_summary = format!("{}\n{}", book.life_summary, folded.join("\n"));
    set_life_summary(book, &new_summary)?;
    book.index.drain(..drop);
    write_index(book)?;
    Ok(())
}

fn page_path(book: &Book, page_id: u64) -> PathBuf {
    book.dir.join("pages").join(format!("{page_id:04}.parquet"))
}

// ---- parquet read/write ----------------------------------------------

fn write_index(book: &Book) -> Result<()> {
    let ids = Int64Array::from(
        book.index.iter().map(|m| m.page_id as i64).collect::<Vec<_>>(),
    );
    let opened = Int64Array::from(
        book.index.iter().map(|m| m.opened_at).collect::<Vec<_>>(),
    );
    let closed = Int64Array::from(
        book.index.iter().map(|m| m.closed_at).collect::<Vec<_>>(),
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
            std::sync::Arc::new(closed),
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

fn read_index(path: &Path) -> Result<Vec<PageMeta>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut metas = Vec::new();
    for batch in reader {
        let batch = batch?;
        let ids = batch.column_by_name("page_id").unwrap();
        let opened = batch.column_by_name("opened_at").unwrap();
        let closed = batch.column_by_name("closed_at").unwrap();
        let topic = batch.column_by_name("topic").unwrap();
        let tags = batch.column_by_name("tags").unwrap();
        let people = batch.column_by_name("people").unwrap();
        let facts = batch.column_by_name("facts").unwrap();
        let plans = batch.column_by_name("plans").unwrap();
        let open = batch.column_by_name("open").unwrap();
        let tokens = batch.column_by_name("life_tokens").unwrap();
        for i in 0..batch.num_rows() {
            metas.push(PageMeta {
                page_id: as_i64(ids, i) as u64,
                opened_at: as_i64(opened, i),
                closed_at: as_i64(closed, i),
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

fn write_page(book: &Book, page_id: u64, messages: &[Message]) -> Result<()> {
    std::fs::create_dir_all(book.dir.join("pages"))?;
    let n = messages.len();
    let ids = Int64Array::from(vec![page_id as i64; n]);
    let seq = Int64Array::from((0..n as i64).collect::<Vec<_>>());
    let role = StringArray::from(
        messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
    );
    let content = StringArray::from(
        messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
    );
    let ts = Int64Array::from(messages.iter().map(|m| m.ts).collect::<Vec<_>>());

    let batch = RecordBatch::try_new(
        page_schema(),
        vec![
            std::sync::Arc::new(ids),
            std::sync::Arc::new(seq),
            std::sync::Arc::new(role),
            std::sync::Arc::new(content),
            std::sync::Arc::new(ts),
        ],
    )?;

    let file = std::fs::File::create(page_path(book, page_id))?;
    let writer = ArrowWriter::try_new(file, page_schema(), None)?;
    let mut writer = writer;
    writer.write(&batch)?;
    let _ = writer.close()?;
    Ok(())
}

fn read_page(path: &Path) -> Result<Vec<Message>> {
    let file = std::fs::File::open(path)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut msgs = Vec::new();
    for batch in reader {
        let batch = batch?;
        let seq = batch.column_by_name("seq").unwrap();
        let role = batch.column_by_name("role").unwrap();
        let content = batch.column_by_name("content").unwrap();
        let ts = batch.column_by_name("ts").unwrap();
        let mut rows: Vec<(i64, String, String, i64)> = Vec::new();
        for i in 0..batch.num_rows() {
            rows.push((
                as_i64(seq, i),
                as_str(role, i),
                as_str(content, i),
                as_i64(ts, i),
            ));
        }
        rows.sort_by_key(|r| r.0);
        for (_, role, content, ts) in rows {
            msgs.push(Message { role, content, ts });
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
    fn new_page_and_close_roundtrip() {
        let dir = tmpdir("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let id = new_page(&mut book);
        let msgs = vec![
            msg("user", "let's plan Lisbon"),
            msg("assistant", "great, May is nice"),
        ];
        close_page(
            &mut book,
            id,
            &msgs,
            &Bookmark {
                topic: "Lisbon trip".to_string(),
                tags: vec!["travel".to_string(), "lisbon".to_string()],
                people: vec!["grandma".to_string()],
                facts: vec!["may dates".to_string()],
                plans: vec!["book flights".to_string()],
                open: vec!["compare prices".to_string()],
            },
        )
        .unwrap();

        // Reload from disk and read the page back.
        let mut book2 = open_book(&dir).unwrap();
        let loaded = load_page(&book2, id).unwrap().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "let's plan Lisbon");
        assert_eq!(render_page(&loaded), "**user:** let's plan Lisbon\n\n**assistant:** great, May is nice\n\n");

        let metas = index_entries(&book2);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].topic, "Lisbon trip");
        assert_eq!(metas[0].tags, vec!["travel", "lisbon"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_pages_ranks_by_keyword_overlap() {
        let dir = tmpdir("match");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let a = new_page(&mut book);
        close_page(
            &mut book,
            a,
            &[msg("user", "lisbon")],
            &Bookmark {
                topic: "Lisbon trip".to_string(),
                tags: vec!["travel".to_string(), "lisbon".to_string()],
                facts: vec!["may".to_string()],
                ..Bookmark::default()
            },
        )
        .unwrap();
        let b = new_page(&mut book);
        close_page(
            &mut book,
            b,
            &[msg("user", "cat")],
            &Bookmark {
                topic: "Miso the cat".to_string(),
                tags: vec!["cat".to_string(), "vet".to_string()],
                facts: vec!["vet tuesday".to_string()],
                ..Bookmark::default()
            },
        )
        .unwrap();

        let hits = match_pages(&book, "lisbon may dates");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].topic, "Lisbon trip");

        let cat = match_pages(&book, "take miso to the vet");
        assert_eq!(cat[0].topic, "Miso the cat");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_folds_old_pages_into_life_summary() {
        let dir = tmpdir("compact");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        for i in 0..5 {
            let id = new_page(&mut book);
            close_page(
                &mut book,
                id,
                &[msg("user", "chat")],
                &Bookmark {
                    topic: format!("topic {i}"),
                    tags: vec!["tag".to_string()],
                    ..Bookmark::default()
                },
            )
            .unwrap();
        }
        assert_eq!(index_entries(&book).len(), 5);
        compact(&mut book, 2).unwrap();
        let metas = index_entries(&book);
        assert_eq!(metas.len(), 2);
        assert!(life_summary(&book).contains("topic 0"));
        // Old content files are gone; new ones remain.
        assert!(load_page(&book, metas[0].page_id).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_ids_are_monotonic_after_reopen() {
        let dir = tmpdir("ids");
        let _ = std::fs::remove_dir_all(&dir);
        let mut book = open_book(&dir).unwrap();
        let id = new_page(&mut book);
        close_page(
            &mut book,
            id,
            &[msg("user", "hi")],
            &Bookmark {
                topic: "hello".to_string(),
                ..Bookmark::default()
            },
        )
        .unwrap();
        drop(book);
        let mut book2 = open_book(&dir).unwrap();
        let id2 = new_page(&mut book2);
        assert!(id2 > id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
