use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Spec {
    OpenAi,
    OpenResponses,
}

type Compiled = Mutex<HashMap<(Spec, String), Arc<jsonschema::Validator>>>;

pub const BOTH: &[Spec] = &[Spec::OpenAi, Spec::OpenResponses];
pub const OPENAI: &[Spec] = &[Spec::OpenAi];
pub const OPEN_RESPONSES: &[Spec] = &[Spec::OpenResponses];

fn any_of_for_one_of(node: &mut Value) {
    match node {
        Value::Object(map) => {
            if let Some(branches) = map.remove("oneOf") {
                map.insert("anyOf".into(), branches);
            }
            map.values_mut().for_each(any_of_for_one_of);
        }
        Value::Array(items) => items.iter_mut().for_each(any_of_for_one_of),
        _ => {}
    }
}

impl Spec {
    fn document(self) -> &'static Value {
        static OPENAI_DOC: OnceLock<Value> = OnceLock::new();
        static OPEN_RESPONSES_DOC: OnceLock<Value> = OnceLock::new();
        let (cell, text) = match self {
            Spec::OpenAi => (&OPENAI_DOC, include_str!("../spec/openai.json")),
            Spec::OpenResponses => (
                &OPEN_RESPONSES_DOC,
                include_str!("../spec/openresponses.json"),
            ),
        };
        cell.get_or_init(|| {
            let mut doc: Value = serde_json::from_str(text).expect("a vendored spec parses");
            any_of_for_one_of(&mut doc);
            doc
        })
    }

    fn schemas(self) -> &'static serde_json::Map<String, Value> {
        self.document()["components"]["schemas"]
            .as_object()
            .expect("a spec has schemas")
    }

    pub fn has(self, schema: &str) -> bool {
        self.schemas().contains_key(schema)
    }

    pub fn event_schema(self, event_type: &str) -> Option<&'static str> {
        static NAMED: OnceLock<HashMap<(Spec, String), String>> = OnceLock::new();
        let named = NAMED.get_or_init(|| {
            let mut named = HashMap::new();
            for spec in [Spec::OpenAi, Spec::OpenResponses] {
                let events: Vec<&String> = match spec {
                    Spec::OpenAi => spec.schemas()["ResponseStreamEvent"]["anyOf"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|r| r["$ref"].as_str())
                        .filter_map(|r| spec.schemas().get_key_value(r.rsplit('/').next()?))
                        .map(|(name, _)| name)
                        .collect(),
                    Spec::OpenResponses => spec
                        .schemas()
                        .keys()
                        .filter(|name| {
                            name.ends_with("StreamingEvent") && !name.starts_with("WebSocket")
                        })
                        .collect(),
                };
                for name in events {
                    let ty = &spec.schemas()[name.as_str()]["properties"]["type"];
                    let consts = ty["enum"].as_array().cloned().unwrap_or_default();
                    for v in consts
                        .iter()
                        .chain(ty.get("const"))
                        .chain(ty.get("default"))
                    {
                        if let Some(v) = v.as_str() {
                            named.insert((spec, v.to_string()), name.clone());
                        }
                    }
                }
            }
            named
        });
        named
            .get(&(self, event_type.to_string()))
            .map(String::as_str)
    }

    pub fn violations(self, schema: &str, value: &Value) -> Vec<String> {
        static COMPILED: OnceLock<Compiled> = OnceLock::new();
        assert!(self.has(schema), "{self:?} has no schema {schema}");
        let validator = COMPILED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry((self, schema.to_string()))
            .or_insert_with(|| {
                let wrapped = json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "$ref": format!("#/components/schemas/{schema}"),
                    "components": self.document()["components"],
                });
                Arc::new(jsonschema::validator_for(&wrapped).expect("a vendored schema compiles"))
            })
            .clone();
        validator
            .iter_errors(value)
            .map(|e| format!("{} at {}", e, e.instance_path()))
            .collect()
    }
}

pub fn conforms(specs: &[Spec], schema: &str, value: &Value) {
    for spec in specs {
        let broken = spec.violations(schema, value);
        assert!(
            broken.is_empty(),
            "not a {schema} of {spec:?}:\n{}\n{value:#}",
            broken.join("\n")
        );
    }
}

pub fn streamed(specs: &[Spec], event: &Value) {
    let ty = event["type"].as_str().expect("an event names its type");
    for spec in specs {
        let schema = spec
            .event_schema(ty)
            .unwrap_or_else(|| panic!("{spec:?} streams no {ty} event"));
        conforms(&[*spec], schema, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finished_item_is_named_item_not_output_item() {
        let misnamed = json!({ "type": "response.output_item.done", "sequence_number": 1, "output_index": 0, "output_item": { "type": "reasoning", "id": "rs_1", "summary": [] } });
        for spec in BOTH {
            let schema = spec.event_schema("response.output_item.done").unwrap();
            assert!(
                !spec.violations(schema, &misnamed).is_empty(),
                "{spec:?} took output_item"
            );
        }
    }

    #[test]
    fn argument_deltas_are_matched_by_item_id() {
        let misnamed = json!({ "type": "response.function_call_arguments.delta", "sequence_number": 1, "output_index": 0, "output_item_id": "fc_1", "delta": "{" });
        for spec in BOTH {
            let schema = spec
                .event_schema("response.function_call_arguments.delta")
                .unwrap();
            assert!(
                !spec.violations(schema, &misnamed).is_empty(),
                "{spec:?} took output_item_id"
            );
        }
    }

    #[test]
    fn every_event_carries_its_sequence_number() {
        let unnumbered = json!({ "type": "response.output_text.delta", "item_id": "m", "output_index": 0, "content_index": 0, "delta": "hi", "logprobs": [] });
        for spec in BOTH {
            let schema = spec.event_schema("response.output_text.delta").unwrap();
            assert!(
                !spec.violations(schema, &unnumbered).is_empty(),
                "{spec:?} took no sequence_number"
            );
        }
    }

    #[test]
    fn a_summary_of_none_is_no_summary_either_spec_takes() {
        let asked = json!({ "model": "m", "input": "hi", "reasoning": { "summary": "none" } });
        assert!(!Spec::OpenAi.violations("CreateResponse", &asked).is_empty());
        assert!(
            !Spec::OpenResponses
                .violations("CreateResponseBody", &asked)
                .is_empty()
        );
    }
}
