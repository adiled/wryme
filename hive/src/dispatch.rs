//! WrymeDispatcher — wryme's tool registry plugged into `serve_forager`.
//!
//! Deliberately thin: the tool surface is already modular in the wryme
//! crate (`tool_defs` for the catalogue, `execute` for the run, `book`
//! for the engine), so this only does wire-shape conversion. Adding a
//! tool to the TUI adds it to the hive for free.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use hum_nest::{ToolDef, ToolDispatcher, ToolResult};
use serde_json::Value;
use wryme::book;

pub(crate) struct WrymeDispatcher {
    engine: Arc<Mutex<book::Engine>>,
}

impl WrymeDispatcher {
    pub(crate) fn new() -> Result<Self> {
        // The hive gets its own book, separate from the TUI's, so two
        // processes never write the same stream segments.
        let engine = book::open_engine(&book_dir())?;
        Ok(Self {
            engine: Arc::new(Mutex::new(engine)),
        })
    }
}

fn book_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("wryme")
        .join("hive")
        .join("book")
}

fn defs_from_json(defs: Vec<Value>) -> Vec<ToolDef> {
    defs.into_iter()
        .filter_map(|v| {
            Some(ToolDef {
                name: v.get("name")?.as_str()?.to_string(),
                description: v.get("description")?.as_str()?.to_string(),
                input_schema: v.get("parameters").cloned().unwrap_or_else(|| {
                    serde_json::json!({ "type": "object", "additionalProperties": false })
                }),
            })
        })
        .collect()
}

#[async_trait]
impl ToolDispatcher for WrymeDispatcher {
    fn tool_defs(&self) -> Vec<ToolDef> {
        defs_from_json(wryme::tools::tool_defs())
    }

    async fn dispatch(&self, tone: Value) -> ToolResult {
        let tool_name = tone
            .get("toolName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let args = tone.get("args").cloned().unwrap_or(Value::Null);
        // wryme's tools take the raw argument string (JSON or bare, they
        // parse it themselves) so the wire can hand us either shape.
        let args_str = match args.as_str() {
            Some(s) => s.to_string(),
            None => args.to_string(),
        };

        match wryme::tools::execute(&self.engine, &tool_name, &args_str).await {
            Some(out) => ToolResult::text(out),
            None => ToolResult::error(format!("wryme: unknown tool {tool_name:?}")),
        }
    }
}
