use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use super::{OpenAiResponsesBackendError as Error, wire};
use crate::inference::InferenceResponse;

const MAX_ITEMS: usize = 1024;
const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
const MAX_PARTS: usize = 4096;

#[derive(Default)]
pub(super) struct ResponseStream {
    items: BTreeMap<u64, Value>,
    started: HashMap<u64, StartedItem>,
    ids: HashMap<String, u64>,
    call_ids: HashMap<String, u64>,
    retained_bytes: usize,
    terminal: Option<Value>,
    unindexed_activity: bool,
    part_count: usize,
}

#[derive(Default)]
struct StartedItem {
    id: Option<String>,
    kind: Option<String>,
    call_id: Option<String>,
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
        let entry = self.started.entry(index).or_default();
        let kind = item.get("type").and_then(Value::as_str).map(str::to_owned);
        if let (Some(old), Some(new)) = (&entry.kind, &kind)
            && old != new
        {
            return Err(Error::OutputIntegrity {
                index: Some(index as usize),
                detail: "output item type changed",
            });
        }
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            if let Some(old) = &entry.id
                && old != id
            {
                return Err(Error::OutputIntegrity {
                    index: Some(index as usize),
                    detail: "output item identity changed",
                });
            }
            if let Some(old_index) = self.ids.insert(id.to_owned(), index)
                && old_index != index
            {
                return Err(Error::OutputIntegrity {
                    index: Some(index as usize),
                    detail: "output item identity reused",
                });
            }
            entry.id = Some(id.to_owned());
        }
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            if let Some(old) = &entry.call_id
                && old != call_id
            {
                return Err(Error::OutputIntegrity {
                    index: Some(index as usize),
                    detail: "call identity changed",
                });
            }
            if let Some(old_index) = self.call_ids.insert(call_id.to_owned(), index)
                && old_index != index
            {
                return Err(Error::OutputIntegrity {
                    index: Some(index as usize),
                    detail: "call identity reused",
                });
            }
            entry.call_id = Some(call_id.to_owned());
        }
        entry.kind = kind;
        if !done {
            if item.get("status").and_then(Value::as_str) == Some("completed") {
                return Err(Error::OutputIntegrity {
                    index: Some(index as usize),
                    detail: "added item already completed",
                });
            }
        } else {
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
            return Ok(());
        }
        if output.len() > MAX_ITEMS {
            return Err(Error::ResourceLimit("too many terminal output items"));
        }
        let terminal_bytes: usize = output.iter().map(|item| item.to_string().len()).sum();
        if terminal_bytes > MAX_ITEM_BYTES {
            return Err(Error::ResourceLimit("terminal output exceeded size limit"));
        }
        for (index, item) in output.iter().enumerate() {
            wire::validate_output_item(item)?;
            if let Some(done) = self.items.get(&(index as u64)) {
                reconcile(done, item, index)?;
            }
        }
        if self.items.len() > output.len() {
            return Err(Error::OutputIntegrity {
                index: None,
                detail: "terminal output omitted finalized items",
            });
        }
        if self.started.len() > self.items.len() {
            return Err(Error::OutputIntegrity {
                index: None,
                detail: "started output item was not finalized",
            });
        }
        if self
            .items
            .keys()
            .any(|index| *index as usize >= output.len())
        {
            return Err(Error::OutputIntegrity {
                index: None,
                detail: "terminal output indexes do not match finalized items",
            });
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

fn reconcile(done: &Value, terminal: &Value, index: usize) -> Result<(), Error> {
    let fields = [
        "type",
        "id",
        "role",
        "name",
        "call_id",
        "arguments",
        "content",
        "summary",
        "encrypted_content",
    ];
    for field in fields {
        let a = done.get(field);
        let b = terminal.get(field);
        if field == "encrypted_content"
            && a.and_then(Value::as_str).is_none()
            && b.and_then(Value::as_str).is_some()
        {
            continue;
        }
        if a != b && !(field == "role" && a.is_none()) {
            return Err(Error::OutputIntegrity {
                index: Some(index),
                detail: "finalized and terminal output conflict",
            });
        }
    }
    Ok(())
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
        assert!(matches!(r.output[0].effect(), crate::inference::OutputEffect::Text(t) if t=="OK"));
    }
    #[test]
    fn rejects_conflicting_terminal_item() {
        let item = message("OK");
        let mut other = message("NO");
        other["id"] = json!("m");
        assert!(stream(vec![json!({"type":"response.output_item.done","output_index":0,"item":item}),json!({"type":"response.completed","response":{"status":"completed","output":[other]}})]).is_err());
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
