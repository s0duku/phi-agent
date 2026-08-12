# Serialization

Phi's internal JSON is a canonical representation of the current Rust types.
Session input is non-empty UTF-8 JSON; creation is a separate operation.
Historical aliases are not part of the current format.

An assistant history message owns its content, reasoning blocks, tool calls,
and provider context. A `RequestExecutor` owns the pending assistant turn and
its `pending_results`. Intermediate successful tools replace this step rather
than serializing repeated executor frames. Completing the batch creates a new
`RequestProvider` frame with the assistant and all tool results in its delta.

Provider-specific request JSON is not Session state. Providers receive
`PhiRenderedMessages`, read the most recent available provider context through
that boundary, and serialize canonical Phi messages into their wire format.
They construct model responses as one assistant message instead of exposing a
second provider-message history type. Provider context may preserve choices
such as `reasoning` versus `reasoning_content` across requests and processes;
without context, OpenAI Chat defaults to `reasoning_content`.

Wire-format tolerance is also a provider responsibility. Required keys are
emitted with non-null empty values when absent: assistant `content` is `""` and
list fields such as tool calls are `[]`. Tool-result strings remain strings;
structured results are encoded once as JSON text where the provider protocol
requires text.

Frame deltas serialize persistent module state as canonical variable effects.
See [Variable effects](variable-effects.md#serialization).
