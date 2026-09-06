# Responses generation result contract

Minuet generation uses the stream-only OpenAI Responses protocol. The
inference adapter owns SSE framing, event validation, output-source selection,
and protocol errors. Runtime receives one immutable result only after a
successful `response.completed` event and commits it before exposing any tool
calls.

The terminal response's complete `output` array is authoritative when present.
Provisional deltas and item events may be incomplete or serialized differently
and cannot override a parseable terminal result. If terminal `output` is
explicitly empty, the adapter may reconstruct the result from a contiguous,
complete set of `response.output_item.done` items observed in the same stream.
Missing or ambiguous evidence is an error only when it prevents that
reconstruction. Final function-call identities remain unique because they
drive tool execution; ordinary item IDs are association and diagnostic data.

Opaque continuation data remains intact for replay. Usage comes from the
validated terminal response and is recorded once with the committed result;
absent or null usage remains unreported. Failed, incomplete, malformed,
truncated, and timed-out streams do not commit the model turn or execute its
tool calls. The adapter owns request-local buffers and releases them on every
success and failure path.

This contract does not require a provider to implement input-token counting;
that operation remains separate. It does not promise support for non-streaming
generation, automatic retries, reconnects, or provider-specific dialects.
