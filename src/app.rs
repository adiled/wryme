// Application state. The UI is a pure function of this.

use std::sync::{Arc, Mutex};

use crate::api::ApiMessage;
use crate::book;
use crate::popup::Popup;
use crate::shop::Shop;
use crate::station::Station;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// What kind of chunk the model most recently sent us during a stream.
/// Drives the dim header indicator next to the role label: "thinking…"
/// vs "writing…" vs "tinkering…". Only meaningful while `streaming`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Streaming,
    Thinking,
    Tinkering,
    Writing,
}

/// How the message area handles overflow.
///
/// Page is the default. Content is shown in discrete viewport-sized chunks
/// and navigation snaps between them. Scroll is the alternative: a smooth
/// row-by-row offset, more like a traditional terminal pager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Page,
    Scroll,
}

#[derive(Debug)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Reasoning / chain-of-thought from the model, if it sent any.
    /// Streamed before `content`, rendered below the reply (older in time)
    /// as a dimmer "brain" block.
    pub brain: String,
    /// True while the model is still streaming into this message.
    pub streaming: bool,
    /// Local time the message was created, formatted "HH:mm". Set once
    /// when the message is pushed and never updated. We don't persist
    /// across sessions, so the date is always today and not stored.
    pub timestamp: String,
    /// What the model is currently doing (writing, thinking, calling a
    /// tool). Drives the dim header label. Stops being displayed once
    /// streaming ends.
    pub phase: Phase,
    /// Name of the tool the model is currently calling, if any. Surfaced
    /// to the right of the phase indicator on the header line, just left
    /// of the timestamp. Only displayed while streaming.
    pub current_tool: Option<String>,
    /// Results of tools we ran locally (myshell_explore) this turn, in
    /// order. Rendered as a dim block under the header, above the final
    /// reply, so the user sees the machine doing the thing.
    pub tool_results: Vec<ToolResult>,
    /// Logical turn this message belongs to. A single assistant reply can
    /// span several messages (one per contiguous non-thinking cluster) —
    /// they all share the same `turn_id` so the wire transcript and the
    /// book record still treat them as one turn. User messages set 0.
    pub turn_id: u64,
}

/// One local tool execution: which tool ran and what it returned.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub name: String,
    pub output: String,
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

pub struct App {
    /// Messages in chronological order (index 0 = oldest).
    /// The UI renders them in reverse so the newest sits at the top.
    pub messages: Vec<Message>,
    pub system: Option<String>,
    /// True while a request is in flight.
    pub in_flight: bool,
    pub status: String,
    pub should_quit: bool,
    /// Which page of the message stack the user is currently viewing.
    /// 0 = the live page (newest content at top). Higher = further back
    /// in the conversation. The renderer clamps this to the number of
    /// pages actually available given the current viewport. Only meaningful
    /// in `ViewMode::Page`.
    pub current_page: usize,
    /// Row-level scroll offset, used only in `ViewMode::Scroll`. 0 = newest
    /// content visible at the top, higher = scrolled into older content.
    pub scroll_row: usize,
    /// Mouse wheel accumulator. Ticks add up here; once the magnitude
    /// crosses a threshold we move a page and subtract the threshold.
    /// Keeps trackpad scrolling from blowing through pages instantly.
    pub wheel_accum: i32,
    /// Current view mode. Page is the default; Scroll is the alternative.
    pub view_mode: ViewMode,
    /// Last rendered viewport height (rows). Recorded by the renderer so
    /// the key handlers can step by viewport when the user hits PgUp/PgDn
    /// in scroll mode.
    pub last_viewport_h: usize,
    /// Most recent response.id seen from a Responses-protocol station.
    /// Used as `previous_response_id` on the next request so the server
    /// pins us to the same warm session (matters for stations that keep
    /// per-session state like cached tool results or MCP server state).
    /// Reset to None on launch; we don't persist across runs.
    pub last_response_id: Option<String>,
    /// All shops loaded at startup. Read-only after that. Used by the
    /// popup to list every model any shop can run.
    pub shops: Vec<Shop>,
    /// Every saved station loaded at startup, plus any the user saves
    /// during this session via the popup. Mutable.
    pub stations: Vec<Station>,
    /// The station currently in effect. The popup mutates this when the
    /// user adjusts dials, picks a different model, or loads a saved
    /// station. Cloned into each in-flight request.
    pub active_station: Station,
    /// The shop currently in effect. Re-resolved every time
    /// active_station.model changes.
    pub active_shop: Shop,
    /// Name of the saved station this session traces back to, if any.
    /// None when the session was synthesized (untitled, demo). Used
    /// alongside `active_station` to compute whether the active config
    /// is "dirty" (modified vs. its saved form).
    pub active_origin: Option<String>,
    /// The popup overlay state (closed, browsing, or entering a name).
    pub popup: Popup,
    /// The book engine: wryme's memory, a columnar Parquet store of
    /// open-ended compartments. Shared across turns as an Arc<Mutex<..>>
    /// so the streaming protocol and the background delivery can both
    /// reach it (the invisible `book` tool locks it in the flow).
    pub engine: Arc<Mutex<book::Engine>>,
    /// Monotonic counter for logical turns; bumped by begin_assistant and
    /// stamped onto every cluster of that turn.
    turn_counter: u64,
    /// True if the last thing streamed into the assistant was thinking
    /// (a Brain delta). A content delta arriving right after this starts
    /// a fresh cluster so each contiguous non-thinking run is its own entry.
    last_stream_was_brain: bool,
}

/// The book lives at `~/.config/wryme/book`, like the shops and
/// stations files — one folder for everything wryme keeps.
fn book_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("wryme")
        .join("book")
}

impl App {
    pub fn new(
        system: Option<String>,
        shops: Vec<Shop>,
        stations: Vec<Station>,
        active_station: Station,
        active_shop: Shop,
        active_origin: Option<String>,
    ) -> Self {
        Self {
            messages: Vec::new(),
            system,
            in_flight: false,
            status: String::new(),
            should_quit: false,
            current_page: 0,
            scroll_row: 0,
            wheel_accum: 0,
            view_mode: ViewMode::Page,
            last_viewport_h: 0,
            last_response_id: None,
            shops,
            stations,
            active_station,
            active_shop,
            active_origin,
            popup: Popup::default(),
            engine: Arc::new(Mutex::new(
                book::open_engine(&book_dir()).expect("open book"),
            )),
            turn_counter: 0,
            last_stream_was_brain: false,
        }
    }


    /// True when the active station differs from the saved entry it was
    /// loaded from. False when there is no origin (untitled / demo) or
    /// when active matches its saved entry exactly.
    pub fn is_dirty(&self) -> bool {
        let Some(origin) = &self.active_origin else {
            return false;
        };
        let Some(saved) = self.stations.iter().find(|s| s.name == *origin) else {
            // Origin set but saved entry missing. Should not happen in
            // normal use; treat as dirty so the user notices.
            return true;
        };
        saved.model != self.active_station.model
            || saved.dials.boldness != self.active_station.dials.boldness
            || saved.dials.patience != self.active_station.dials.patience
            || saved.dials.verbosity != self.active_station.dials.verbosity
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    pub fn push_user(&mut self, content: String) {
        self.messages.push(Message {
            role: Role::User,
            content,
            brain: String::new(),
            streaming: false,
            timestamp: now_hhmm(),
            phase: Phase::Streaming,
            current_tool: None,
            tool_results: Vec::new(),
            turn_id: 0,
        });
        // The engine records every turn into the continuous stream, and
        // re-checks whether an unattributed thread is weightful enough to
        // prod the agent into deeming it.
        if let Some(last) = self.messages.last() {
            if last.role == Role::User {
                if let Ok(mut e) = self.engine.lock() {
                    e.record_turn("user", &last.content);
                }
            }
        }
    }

    pub fn begin_assistant(&mut self) {
        self.turn_counter += 1;
        let tid = self.turn_counter;
        self.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            brain: String::new(),
            streaming: true,
            timestamp: now_hhmm(),
            phase: Phase::Streaming,
            current_tool: None,
            tool_results: Vec::new(),
            turn_id: tid,
        });
    }

    pub fn append_to_last_assistant(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let idx = self
            .messages
            .iter()
            .rposition(|m| m.role == Role::Assistant && m.streaming);
        let Some(idx) = idx else { return };

        // A writing delta right after thinking starts a fresh cluster, so
        // each contiguous non-thinking run shows as its own assistant entry.
        // (An empty placeholder that already holds the leading brain just
        // absorbs the first content — that is still the first cluster.)
        if self.last_stream_was_brain && !self.messages[idx].content.is_empty() {
            let tid = self.messages[idx].turn_id;
            // Close the previous cluster: only the LATEST cluster of a turn
            // stays `streaming`, so the phase label, tool name and cursor
            // show on the newest entry only. Older clusters render as
            // finished text without live indicators.
            self.messages[idx].streaming = false;
            self.messages.push(Message {
                role: Role::Assistant,
                content: String::new(),
                brain: String::new(),
                streaming: true,
                timestamp: now_hhmm(),
                phase: Phase::Streaming,
                current_tool: None,
                tool_results: Vec::new(),
                turn_id: tid,
            });
        }
        self.last_stream_was_brain = false;

        let m = self.messages.last_mut().unwrap();
        // Don't jam a chunk against the previous one: the model frequently
        // splits "…." and the next word across two deltas, producing
        // "sentence.Next" with no space. Insert one only when both sides
        // are non-whitespace so we never double spaces or mangle markdown.
        if !m.content.is_empty()
            && !delta.starts_with(char::is_whitespace)
            && !m.content.ends_with(char::is_whitespace)
        {
            m.content.push(' ');
        }
        m.content.push_str(delta);
        m.phase = Phase::Writing;
    }

    pub fn append_to_last_brain(&mut self, delta: &str) {
        // All reasoning of a turn lands on ONE message — the turn's first
        // cluster — so the brain shows as a single UI entry per turn rather
        // than a footnote on every cluster.
        let tid = self.turn_counter;
        if let Some(m) = self
            .messages
            .iter_mut()
            .find(|m| m.role == Role::Assistant && m.turn_id == tid)
        {
            m.brain.push_str(delta);
        }
        // The "thinking…" indicator belongs on the latest visual entry only.
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.phase = Phase::Thinking;
        }
        self.last_stream_was_brain = true;
    }

    pub fn record_tool_call(&mut self, name: Option<String>) {
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.phase = Phase::Tinkering;
            if let Some(n) = name {
                m.current_tool = Some(n);
            }
        }
    }

    pub fn append_tool_result(&mut self, name: String, output: String) {
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.phase = Phase::Tinkering;
            m.tool_results.push(ToolResult { name, output });
        }
    }

    pub fn finish_streaming(&mut self) {
        self.in_flight = false;

        // Find the streaming assistant message (most recent) and its turn_id.
        let mut just_finished: Option<usize> = None;
        for i in (0..self.messages.len()).rev() {
            if self.messages[i].streaming {
                just_finished = Some(i);
                break;
            }
        }

        if let Some(i) = just_finished {
            let tid = self.messages[i].turn_id;

            // Mark every cluster of this turn done.
            for m in self.messages.iter_mut() {
                if m.streaming && m.turn_id == tid {
                    m.streaming = false;
                }
            }

            // Record the whole logical turn (all clusters) into the book once.
            let joined: String = self.messages
                .iter()
                .filter(|m| m.role == Role::Assistant && m.turn_id == tid && !m.content.is_empty())
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !joined.is_empty() {
                if let Ok(mut e) = self.engine.lock() {
                    e.record_turn("assistant", &joined);
                }
            }

            // Drop empty clusters of this turn so the screen does not show a
            // confusing empty bubble. Server hiccups and pre-delta errors are
            // common causes. If upstream sent an error, the status bar already
            // explains what happened. If not, leave a short note.
            let any_nonempty = self.messages
                .iter()
                .any(|m| m.role == Role::Assistant && m.turn_id == tid
                    && (!m.content.is_empty() || !m.brain.is_empty()
                        || m.current_tool.is_some() || !m.tool_results.is_empty()));
            if !any_nonempty {
                self.messages.retain(|m| !(m.role == Role::Assistant && m.turn_id == tid));
                if self.status.is_empty() {
                    self.note("empty reply");
                }
            }
        }
    }

    /// Build the wire-format message list to send upstream. The base
    /// system prompt, then the established compartments' rendered
    /// bookmarks as one system message each — the preamble — then the
    /// live turns.
    pub fn api_messages(&self) -> Vec<ApiMessage> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        if let Some(sys) = &self.system {
            out.push(ApiMessage {
                role: "system".into(),
                content: sys.clone(),
            });
        }
        if let Ok(mut engine) = self.engine.lock() {
            for preamble in engine.preamble() {
                out.push(ApiMessage {
                    role: "system".into(),
                    content: preamble,
                });
            }
            // The book-writing prod: a quiet system reminder the engine
            // slips the agent once when an unattributed thread is weightful.
            if let Some(prod) = engine.take_prod() {
                out.push(ApiMessage {
                    role: "system".into(),
                    content: prod,
                });
            }
        }
        let mut last_asst_turn: Option<u64> = None;
        for m in &self.messages {
            // Skip an empty streaming placeholder. We send the history
            // BEFORE the assistant turn we're about to fill.
            if m.streaming && m.content.is_empty() {
                continue;
            }
            // A cluster of an assistant turn still in flight: skip it too —
            // we only send history up to the turn we're about to fill.
            if m.role == Role::Assistant && m.streaming {
                continue;
            }
            // Consecutive assistant clusters of the SAME logical turn merge
            // into one ApiMessage, so the wire sees one assistant turn.
            if m.role == Role::Assistant
                && last_asst_turn == Some(m.turn_id)
                && out.last().map(|a| a.role.as_str()) == Some("assistant")
            {
                let last = out.last_mut().unwrap();
                if !last.content.is_empty() && !m.content.is_empty() {
                    last.content.push('\n');
                }
                last.content.push_str(&m.content);
                last_asst_turn = Some(m.turn_id);
                continue;
            }
            if m.role == Role::Assistant {
                last_asst_turn = Some(m.turn_id);
            }
            out.push(ApiMessage {
                role: match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
                .into(),
                content: m.content.clone(),
            });
        }
        out
    }
}
