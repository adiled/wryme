// Application state. The UI is a pure function of this.

use crate::api::ApiMessage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
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
    /// Byte offset in `content` where the most recent delta begins. The
    /// renderer uses this to center-spawn the latest chunk in the input
    /// stream while the first line is still building. Updated on every
    /// append. Only meaningful while `streaming` is true.
    pub last_delta_byte: usize,
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
}

impl App {
    pub fn new(system: Option<String>) -> Self {
        Self {
            messages: Vec::new(),
            system,
            in_flight: false,
            status: String::new(),
            should_quit: false,
        }
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
            last_delta_byte: 0,
        });
    }

    pub fn begin_assistant(&mut self) {
        self.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            brain: String::new(),
            streaming: true,
            last_delta_byte: 0,
        });
    }

    pub fn append_to_last_assistant(&mut self, delta: &str) {
        if let Some(m) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.role == Role::Assistant && m.streaming)
        {
            m.last_delta_byte = m.content.len();
            m.content.push_str(delta);
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
        }
    }

    pub fn finish_streaming(&mut self) {
        for m in self.messages.iter_mut().rev() {
            if m.streaming {
                m.streaming = false;
                break;
            }
        }
        self.in_flight = false;
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
