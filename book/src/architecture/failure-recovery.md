# Failure and recovery

Failures are represented as explicit failed steps. `RuntimeFailed` discards the
in-flight delta and creates an empty-delta failed frame whose parent is the
unchanged step that failed. A failed frame never replaces that step.

Recovery modules may then choose a bounce according to the failure and governance
policy.

Provider responses preserve two model-output failures as distinct runtime error
kinds: `ModelOutputLimit` means the provider stopped generation at the configured
output-token limit, while `ModelToolParseError` means a returned tool call had
malformed JSON arguments. Both are still wrapped by `RuntimeFailed` and follow
the normal failed-step fallback until a recovery policy explicitly handles them.

Tool failures carry the assistant turn, completed results, failing request, and
remaining requests needed to explain the failure. Manual recovery first reads
`session state`, then rolls back exactly one frame to restore the parent
`RequestExecutor`, and finally supplies the external result with
`session tool-result`. If earlier tools in the same batch succeeded, their
`pending_results` remain in that restored executor.
