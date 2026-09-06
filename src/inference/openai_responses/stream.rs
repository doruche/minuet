use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use super::{OpenAiResponsesBackendError as Error, wire};
use crate::inference::InferenceResponse;

const MAX_ITEMS: usize = 1024;
const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
const MAX_PARTS: usize = 4096;

#[derive(Default)]
pub(super) struct ResponseStream {
    items: BTreeMap<u64, Value>,
    started: HashSet<u64>,
    retained_bytes: usize,
    terminal: Option<Value>,
    unindexed_activity: bool,
    part_count: usize,
}

impl ResponseStream {
    pub(super) fn push(&mut self, data: &str) -> Result<Option<String>, Error> {
        // The sentinel is framing, not a response-success signal.
        if data == "[DONE]" {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_str(data).map_err(|_| Error::MalformedStream("invalid SSE JSON"))?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if self.terminal.is_some() {
            return Err(Error::MalformedStream(
                "event received after response terminal",
            ));
        }
        match event_type {
            "response.output_text.delta" => {
                self.observe_activity(&value, "message")?;
                Ok(value
                    .get("delta")
                    .and_then(Value::as_str)
                    .map(str::to_owned))
            },
            "response.function_call_arguments.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_content.delta" => {
                let kind = if event_type.starts_with("response.function_call") {
                    "function_call"
                } else {
                    "reasoning"
                };
                self.observe_activity(&value, kind)?;
                Ok(None)
            },
            "response.output_item.added" => {
                let index = index(&value)?;
                let item = value
                    .get("item")
                    .ok_or(Error::MalformedResponse("output item"))?;
                self.observe_item(index, item, false)?;
                Ok(None)
            },
            "response.output_item.done" => {
                let index = index(&value)?;
                let item = value
                    .get("item")
                    .cloned()
                    .ok_or(Error::MalformedResponse("output item"))?;
                self.observe_item(index, &item, true)?;
                self.add_item(index, item)?;
                Ok(None)
            },
            "response.completed" => {
                let response = value
                    .get("response")
                    .cloned()
                    .ok_or(Error::MalformedResponse("completed response"))?;
                self.validate_terminal(&response)?;
                self.terminal = Some(response);
                Ok(None)
            },
            "response.failed" | "response.incomplete" => {
                let response = value.get("response").unwrap_or(&value);
                Err(Error::StreamTerminal(wire::upstream_message(response)))
            },
            "error" => Err(Error::StreamTerminal(wire::upstream_message(&value))),
            _ => Ok(None),
        }
    }

    fn observe_activity(&mut self, event: &Value, kind: &str) -> Result<(), Error> {
        let Some(index) = event.get("output_index").and_then(Value::as_u64) else {
            self.unindexed_activity = true;
            return Ok(());
        };
        self.part_count += 1;
        if self.part_count > MAX_PARTS {
            return Err(Error::ResourceLimit("too many output parts"));
        }
        let item =
            serde_json::json!({"type": kind, "id": event.get("item_id").and_then(Value::as_str)});
        self.observe_item(index, &item, false)
    }

    fn observe_item(&mut self, index: u64, item: &Value, done: bool) -> Result<(), Error> {
        if index > (MAX_ITEMS as u64 - 1) {
            return Err(Error::ResourceLimit("output index exceeds limit"));
        }
        self.started.insert(index);
        if done {
            wire::validate_output_item(item)?;
        }
        Ok(())
    }

    fn add_item(&mut self, index: u64, item: Value) -> Result<(), Error> {
        if self.items.contains_key(&index) {
            return Err(Error::MalformedStream("duplicate output item"));
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(Error::ResourceLimit("too many output items"));
        }
        self.retained_bytes = self.retained_bytes.saturating_add(item.to_string().len());
        if self.retained_bytes > MAX_ITEM_BYTES {
            return Err(Error::ResourceLimit("output items exceeded size limit"));
        }
        self.items.insert(index, item);
        Ok(())
    }

    fn validate_terminal(&self, response: &Value) -> Result<(), Error> {
        if response.get("status").and_then(Value::as_str) != Some("completed") {
            return Err(Error::Incomplete(wire::upstream_message(response)));
        }
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or(Error::MalformedResponse("missing output array"))?;
        if output.is_empty() {
            if self.unindexed_activity {
                return Err(Error::OutputIntegrity {
                    index: None,
                    detail: "unindexed activity cannot reconstruct output",
                });
            }
            if self.started.len() > self.items.len() {
                return Err(Error::OutputIntegrity {
                    index: None,
                    detail: "started output item was not finalized",
                });
            }
            if self
                .started
                .iter()
                .any(|index| !self.items.contains_key(index))
            {
                return Err(Error::OutputIntegrity {
                    index: None,
                    detail: "started output item was not finalized",
                });
            }
            return Ok(());
        }
        if output.len() > MAX_ITEMS {
            return Err(Error::ResourceLimit("too many terminal output items"));
        }
        let terminal_bytes: usize = output.iter().map(|item| item.to_string().len()).sum();
        if terminal_bytes > MAX_ITEM_BYTES {
            return Err(Error::ResourceLimit("terminal output exceeded size limit"));
        }
        for item in output {
            wire::validate_output_item(item)?;
            // The completed response is the authoritative representation. A
            // provider may serialize the same item differently in provisional
            // done events (for example null versus an empty annotations list).
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<InferenceResponse, Error> {
        let mut response = self.terminal.ok_or(Error::MalformedStream(
            "stream ended without response.completed",
        ))?;
        if response["output"].as_array().is_some_and(Vec::is_empty) && !self.items.is_empty() {
            if self
                .items
                .keys()
                .enumerate()
                .any(|(expected, index)| *index != expected as u64)
            {
                return Err(Error::OutputIntegrity {
                    index: None,
                    detail: "output indexes were not contiguous",
                });
            }
            response["output"] = Value::Array(self.items.into_values().collect());
        }
        wire::parse_response(response)
    }
}

fn index(value: &Value) -> Result<u64, Error> {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .ok_or(Error::MalformedResponse("output_index"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(text: &str) -> Value {
        json!({"type":"message","id":"m","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]})
    }
    fn stream(events: Vec<Value>) -> Result<InferenceResponse, Error> {
        let mut s = ResponseStream::default();
        for e in events {
            s.push(&e.to_string())?;
        }
        s.finish()
    }
    #[test]
    fn reconstructs_empty_terminal_from_done_item() {
        let item = message("OK");
        let r = stream(vec![
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
            json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        ])
        .unwrap();
        assert!(
            matches!(r.output[0].effect(), crate::inference::OutputEffect::Message(t) if t=="OK")
        );
    }

    #[test]
    fn complete_and_reconstructed_turns_preserve_message_boundaries_and_call_order() {
        use crate::inference::OutputEffect;
        let items = vec![
            json!({"type":"reasoning","encrypted_content":"opaque"}),
            message("[cross][ref]\n\n"),
            message("[ref]: https://example.test\n\nseparate"),
            json!({"type":"function_call","call_id":"x","name":"echo","arguments":"{}"}),
            message("```rust\nfn partial() {}"),
            json!({"type":"function_call","call_id":"y","name":"echo","arguments":"{}"}),
            message("**after-fence**"),
        ];
        for reconstruct in [false, true] {
            let mut events = Vec::new();
            if reconstruct {
                // Completion arrival order must not replace output_index order.
                for (index, item) in items.iter().enumerate().rev() {
                    events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
                }
            }
            events.push(json!({"type":"response.completed","response":{"status":"completed","output":if reconstruct { vec![] } else { items.clone() }}}));
            let response = stream(events).unwrap();
            assert_eq!(response.output.len(), items.len());
            for (projected, original) in response.output.iter().zip(&items) {
                assert_eq!(
                    projected.clone().into_continuation().protocol_value(),
                    original
                );
            }
            assert!(matches!(response.output[0].effect(), OutputEffect::None));
            assert!(
                matches!(response.output[1].effect(), OutputEffect::Message(text) if text == "[cross][ref]\n\n")
            );
            assert!(
                matches!(response.output[2].effect(), OutputEffect::Message(text) if text.starts_with("[ref]:"))
            );
            assert!(
                matches!(response.output[3].effect(), OutputEffect::ToolCall(call) if call.call_id == "x")
            );
            assert!(
                matches!(response.output[4].effect(), OutputEffect::Message(text) if text.starts_with("```"))
            );
            assert!(
                matches!(response.output[5].effect(), OutputEffect::ToolCall(call) if call.call_id == "y")
            );
            assert!(
                matches!(response.output[6].effect(), OutputEffect::Message(text) if text == "**after-fence**")
            );
        }
    }
    #[test]
    fn terminal_item_is_authoritative() {
        let item = message("OK");
        let mut other = message("NO");
        other["id"] = json!("m");
        let result = stream(vec![
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
            json!({"type":"response.completed","response":{"status":"completed","output":[other]}}),
        ])
        .unwrap();
        assert!(
            matches!(result.output[0].effect(), crate::inference::OutputEffect::Message(t) if t == "NO")
        );
    }

    #[test]
    fn complete_terminal_output_survives_unfinished_preview() {
        let item = message("OK");
        let result = stream(vec![
            json!({"type":"response.output_item.added","output_index":3,"item":{"type":"message","status":"in_progress"}}),
            json!({"type":"response.completed","response":{"status":"completed","output":[item]}}),
        ]).unwrap();
        assert!(
            matches!(result.output[0].effect(), crate::inference::OutputEffect::Message(t) if t == "OK")
        );
    }
    #[test]
    fn rejects_events_after_terminal() {
        assert!(
            stream(vec![
                json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
                json!({"type":"response.output_text.delta","delta":"late"})
            ])
            .is_err()
        );
    }
    #[test]
    fn accepts_done_sentinel_only_as_framing_after_success() {
        let mut s = ResponseStream::default();
        s.push(r#"{"type":"response.completed","response":{"status":"completed","output":[]}}"#)
            .unwrap();
        s.push("[DONE]").unwrap();
        assert!(s.finish().is_ok());
    }
    #[test]
    fn rejects_unindexed_preview_when_terminal_output_is_empty() {
        assert!(
            stream(vec![
                json!({"type":"response.output_text.delta","delta":"preview"}),
                json!({"type":"response.completed","response":{"status":"completed","output":[]}})
            ])
            .is_err()
        );
    }
    #[test]
    fn rejects_a_started_index_that_has_no_matching_finalized_item() {
        let item = message("OK");
        assert!(stream(vec![
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","status":"in_progress"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":item}),
            json!({"type":"response.completed","response":{"status":"completed","output":[]}}),
        ]).is_err());
    }
    #[test]
    fn preserves_tool_call_identity() {
        let item =
            json!({"type":"function_call","id":"f","call_id":"c","name":"x","arguments":"{}"});
        assert!(
            stream(vec![
                json!({"type":"response.output_item.done","output_index":0,"item":item.clone()}),
                json!({"type":"response.output_item.done","output_index":1,"item":item}),
                json!({"type":"response.completed","response":{"status":"completed","output":[]}})
            ])
            .is_err()
        );
    }
}
