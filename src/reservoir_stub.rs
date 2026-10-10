pub const STATIC_BOT_SCORE: u32 = 70;

#[derive(Debug, Default)]
pub struct Reservoir;

impl Reservoir {
    pub fn load() -> Self {
        Self
    }

    pub fn turn_started(&mut self) {}

    pub fn note_round(&mut self, _station: &str, _model: &str, _input: u64, _output: u64, _full: bool) {
    }

    pub fn end_turn(&mut self, _user_text: &str, _brain_text: &str, _tool_chars: u64) {}

    pub fn note_error(&mut self, _station: &str, _message: &str) -> Option<String> {
        None
    }

    pub fn dip(&self, _station: &str) -> Option<f64> {
        None
    }

    pub fn static_figure(&self) -> u32 {
        0
    }
}
