use std::sync::{Arc, Mutex};

use crate::api::{ApiMessage, ApiToolCall};
use crate::book;
use crate::popup::Popup;
use crate::reservoir::Reservoir;
use crate::shop::Shop;
use crate::station::Station;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Streaming,
    Thinking,
    Tinkering,
    Writing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Page,
    Scroll,
}

#[derive(Debug, Clone)]
pub struct ToolEvent {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub result: String,
}

#[derive(Debug)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub images: Vec<String>,
    pub brain: String,
    pub streaming: bool,
    pub timestamp: String,
    pub phase: Phase,
    pub current_tool: Option<String>,
    pub turn_id: u64,
    pub tool_events: Vec<ToolEvent>,
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

pub struct App {
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub in_flight: bool,
    pub status: String,
    pub should_quit: bool,
    pub current_page: usize,
    pub scroll_row: usize,
    pub wheel_accum: i32,
    pub view_mode: ViewMode,
    pub last_viewport_h: usize,
    pub last_response_id: Option<String>,
    pub usage_ctx: u64,
    pub usage_out: u64,
    pub voice_on: bool,
    pub voice_muted: bool,
    pub voice_speaker: Option<crate::voice::Speaker>,
    pub voice_buffer: String,
    pub shops: Vec<Shop>,
    pub stations: Vec<Station>,
    pub active_station: Station,
    pub active_shop: Shop,
    pub active_origin: Option<String>,
    pub popup: Popup,
    pub engine: Arc<Mutex<book::Engine>>,
    pub reservoir: Arc<Mutex<Reservoir>>,
    turn_counter: u64,
    last_stream_was_brain: bool,
}

fn api_tool_call(ev: &ToolEvent) -> ApiToolCall {
    ApiToolCall {
        id: ev.call_id.clone(),
        name: ev.name.clone(),
        arguments: ev.arguments.clone(),
    }
}

fn api_tool_result(ev: &ToolEvent) -> ApiMessage {
    ApiMessage {
        role: "tool".into(),
        content: ev.result.clone(),
        images: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: ev.call_id.clone(),
        tool_result: ev.result.clone(),
    }
}

fn system_message(content: String) -> ApiMessage {
    ApiMessage {
        role: "system".into(),
        content,
        images: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: String::new(),
        tool_result: String::new(),
    }
}

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
            usage_ctx: 0,
            usage_out: 0,
            voice_on: false,
            voice_muted: false,
            voice_speaker: None,
            voice_buffer: String::new(),
            shops,
            stations,
            active_station,
            active_shop,
            active_origin,
            popup: Popup::default(),
            engine: Arc::new(Mutex::new(
                book::open_engine(&book_dir()).expect("open book"),
            )),
            reservoir: Arc::new(Mutex::new(Reservoir::load())),
            turn_counter: 0,
            last_stream_was_brain: false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        let Some(origin) = &self.active_origin else {
            return false;
        };
        let Some(saved) = self.stations.iter().find(|s| s.name == *origin) else {
            return true;
        };
        saved.model != self.active_station.model
            || saved.dials != self.active_station.dials
            || saved.voice != self.active_station.voice
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    pub fn stop_voice(&mut self) {
        if let Some(speaker) = self.voice_speaker.as_mut() {
            speaker.stop();
        }
        self.voice_buffer.clear();
    }

    pub fn mute_voice(&mut self) {
        self.stop_voice();
        self.voice_muted = true;
    }

    pub fn unmute_voice(&mut self) {
        self.voice_muted = false;
    }

    pub fn voice_is_active(&self) -> bool {
        self.voice_speaker
            .as_ref()
            .map(|s| s.is_active())
            .unwrap_or(false)
    }

    pub fn shutdown_voice(&mut self) {
        if let Some(mut speaker) = self.voice_speaker.take() {
            speaker.shutdown();
        }
        self.voice_buffer.clear();
    }

    pub fn ensure_speaker(&mut self, voice: Option<String>) {
        let same = self
            .voice_speaker
            .as_ref()
            .map(|s| s.name == voice)
            .unwrap_or(false);
        if !same {
            self.shutdown_voice();
            self.voice_speaker = Some(crate::voice::Speaker::new(voice));
        }
    }

    pub fn push_user(&mut self, content: String, images: Vec<String>) {
        self.messages.push(Message {
            role: Role::User,
            content,
            images,
            brain: String::new(),
            streaming: false,
            timestamp: now_hhmm(),
            phase: Phase::Streaming,
            current_tool: None,
            turn_id: 0,
            tool_events: Vec::new(),
        });
        if let Some(last) = self.messages.last()
            && last.role == Role::User
            && let Ok(mut e) = self.engine.lock()
        {
            e.record_turn("user", &last.content);
        }
    }

    fn streaming_assistant(&mut self) -> Option<&mut Message> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
    }

    fn open_assistant_cluster(turn_id: u64) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            images: Vec::new(),
            brain: String::new(),
            streaming: true,
            timestamp: now_hhmm(),
            phase: Phase::Streaming,
            current_tool: None,
            turn_id,
            tool_events: Vec::new(),
        }
    }

    pub fn begin_assistant(&mut self) {
        self.turn_counter += 1;
        let tid = self.turn_counter;
        self.messages.push(Self::open_assistant_cluster(tid));
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

        if self.last_stream_was_brain && !self.messages[idx].content.is_empty() {
            let tid = self.messages[idx].turn_id;
            self.messages[idx].streaming = false;
            self.messages.push(Self::open_assistant_cluster(tid));
        }
        self.last_stream_was_brain = false;

        let m = match self.messages.last_mut() {
            Some(m) => m,
            None => return,
        };
        m.content.push_str(delta);
        m.phase = Phase::Writing;
    }

    pub fn append_to_last_brain(&mut self, delta: &str) {
        let tid = self.turn_counter;
        if let Some(m) = self
            .messages
            .iter_mut()
            .find(|m| m.role == Role::Assistant && m.turn_id == tid)
        {
            m.brain.push_str(delta);
        }
        if let Some(m) = self.streaming_assistant() {
            m.phase = Phase::Thinking;
        }
        self.last_stream_was_brain = true;
    }

    pub fn record_tool_call(&mut self, name: Option<String>) {
        if let Some(m) = self.streaming_assistant() {
            m.phase = Phase::Tinkering;
            if let Some(n) = name {
                m.current_tool = Some(n);
            }
        }
    }

    pub fn record_tool_result(
        &mut self,
        call_id: String,
        name: String,
        arguments: String,
        result: String,
    ) {
        if let Some(m) = self.streaming_assistant() {
            m.tool_events.push(ToolEvent {
                call_id,
                name,
                arguments,
                result,
            });
        }
    }

    pub fn finish_streaming(&mut self) {
        self.in_flight = false;

        let just_finished = self.messages.iter().rposition(|m| m.streaming);

        if let Some(i) = just_finished {
            let tid = self.messages[i].turn_id;

            for m in self.messages.iter_mut() {
                if m.streaming && m.turn_id == tid {
                    m.streaming = false;
                }
            }

            let joined: String = self
                .messages
                .iter()
                .filter(|m| m.role == Role::Assistant && m.turn_id == tid && !m.content.is_empty())
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !joined.is_empty()
                && let Ok(mut e) = self.engine.lock()
            {
                e.record_turn("assistant", &joined);
            }

            let user_text = self
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.as_str())
                .unwrap_or("");
            let brain_text: String = self
                .messages
                .iter()
                .filter(|m| m.role == Role::Assistant && m.turn_id == tid && !m.brain.is_empty())
                .map(|m| m.brain.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let tool_chars: u64 = self
                .messages
                .iter()
                .filter(|m| m.role == Role::Assistant && m.turn_id == tid)
                .map(|m| {
                    m.tool_events
                        .iter()
                        .map(|t| t.arguments.len() + t.result.len())
                        .sum::<usize>() as u64
                })
                .sum();
            if let Ok(mut res) = self.reservoir.lock() {
                res.end_turn(user_text, &brain_text, tool_chars);
            }

            let any_nonempty = self.messages.iter().any(|m| {
                m.role == Role::Assistant
                    && m.turn_id == tid
                    && (!m.content.is_empty() || !m.brain.is_empty() || m.current_tool.is_some())
            });
            if !any_nonempty {
                self.messages
                    .retain(|m| !(m.role == Role::Assistant && m.turn_id == tid));
                if self.status.is_empty() {
                    self.note("empty reply");
                }
            }
        }
    }

    pub fn api_messages(&self) -> Vec<ApiMessage> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        if let Some(sys) = &self.system {
            out.push(system_message(sys.clone()));
        }
        if let Ok(mut engine) = self.engine.lock() {
            for preamble in engine.preamble() {
                out.push(system_message(preamble));
            }
            if let Some(prod) = engine.take_prod() {
                out.push(system_message(prod));
            }
        }
        let mut last_asst_turn: Option<u64> = None;
        for m in &self.messages {
            if m.streaming {
                continue;
            }
            if m.role == Role::Assistant
                && last_asst_turn == Some(m.turn_id)
                && out.last().map(|a| a.role.as_str()) == Some("assistant")
                && let Some(last) = out.last_mut()
            {
                if !last.content.is_empty() && !m.content.is_empty() {
                    last.content.push('\n');
                }
                last.content.push_str(&m.content);
                for ev in &m.tool_events {
                    last.tool_calls.push(api_tool_call(ev));
                }
                out.extend(m.tool_events.iter().map(api_tool_result));
                last_asst_turn = Some(m.turn_id);
                continue;
            }
            if m.role == Role::Assistant {
                last_asst_turn = Some(m.turn_id);
            }
            let (tool_calls, mut results) = if m.role == Role::Assistant {
                (
                    m.tool_events.iter().map(api_tool_call).collect(),
                    m.tool_events.iter().map(api_tool_result).collect(),
                )
            } else {
                (Vec::new(), Vec::new())
            };
            out.push(ApiMessage {
                role: match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
                .into(),
                content: m.content.clone(),
                images: if m.role == Role::User {
                    m.images.clone()
                } else {
                    Vec::new()
                },
                tool_calls,
                tool_call_id: String::new(),
                tool_result: String::new(),
            });
            out.append(&mut results);
        }
        let clip = self.active_station.dials.tinker_clip;
        let total_pairs = out.iter().filter(|m| m.role == "tool").count();
        let keep_n = self.active_station.dials.tinker_keep.keep_n(total_pairs);
        if keep_n.is_some() || clip != crate::station::TinkerVal::All {
            if let Some(n) = keep_n
                && total_pairs > n
            {
                let mut to_drop = total_pairs - n;
                let mut pruned: Vec<ApiMessage> = Vec::with_capacity(out.len());
                for msg in out {
                    if msg.role == "tool" && to_drop > 0 {
                        to_drop -= 1;
                        if let Some(last) = pruned.last_mut()
                            && last.role == "assistant"
                            && !last.tool_calls.is_empty()
                        {
                            let id = &msg.tool_call_id;
                            last.tool_calls.retain(|c| c.id != *id);
                        }
                        continue;
                    }
                    pruned.push(msg);
                }
                out = pruned;
            }
            if clip != crate::station::TinkerVal::All {
                for m in &mut out {
                    if m.role == "tool" {
                        let clipped = clip.clip(&m.content);
                        m.content = clipped.clone();
                        m.tool_result = clipped;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new(
            None,
            vec![Shop::demo()],
            vec![Station::demo()],
            Station::demo(),
            Shop::demo(),
            None,
        );
        let dir = std::env::temp_dir().join(format!("wryme_app_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        app.engine = Arc::new(Mutex::new(book::open_engine(&dir).unwrap()));
        app
    }

    #[test]
    fn stream_deltas_concatenate_verbatim() {
        let mut app = test_app();
        app.begin_assistant();
        for d in ["Hello", ",", " world", "!", " un", "der"] {
            app.append_to_last_assistant(d);
        }
        let joined: String = app
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, "Hello, world! under");
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("wryme_app_{}", std::process::id())),
        );
    }
}
