// Application state. The UI is a pure function of this.

use crate::api::ApiMessage;
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
        });
    }

    pub fn begin_assistant(&mut self) {
        self.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            brain: String::new(),
            streaming: true,
            timestamp: now_hhmm(),
            phase: Phase::Streaming,
            current_tool: None,
        });
    }

    pub fn append_to_last_assistant(&mut self, delta: &str) {
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.content.push_str(delta);
            m.phase = Phase::Writing;
        }
    }

    pub fn append_to_last_brain(&mut self, delta: &str) {
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.brain.push_str(delta);
            m.phase = Phase::Thinking;
        }
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

    pub fn finish_streaming(&mut self) {
        self.in_flight = false;

        // Find the streaming assistant message (most recent) and mark it done.
        let mut just_finished: Option<usize> = None;
        for i in (0..self.messages.len()).rev() {
            if self.messages[i].streaming {
                self.messages[i].streaming = false;
                just_finished = Some(i);
                break;
            }
        }

        // If that turn produced nothing visible at all (no content, no brain,
        // no tool indicator), drop it so the screen does not show a confusing
        // empty bubble. Server hiccups and pre-delta errors are common causes.
        // If the upstream sent an error, the status bar already explains
        // what happened. If not, leave a short note so the user knows
        // something landed but came back empty.
        if let Some(i) = just_finished {
            let m = &self.messages[i];
            if m.role == Role::Assistant
                && m.content.is_empty()
                && m.brain.is_empty()
                && m.current_tool.is_none()
            {
                self.messages.remove(i);
                if self.status.is_empty() {
                    self.note("empty reply");
                }
            }
        }
    }

    /// Build the wire-format message list to send upstream.
    pub fn api_messages(&self) -> Vec<ApiMessage> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        if let Some(sys) = &self.system {
            out.push(ApiMessage {
                role: "system".into(),
                content: sys.clone(),
            });
        }
        for m in &self.messages {
            // Skip an empty streaming placeholder. We send the history
            // BEFORE the assistant turn we're about to fill.
            if m.streaming && m.content.is_empty() {
                continue;
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
