# Failure and recovery

Failures are represented as explicit failed steps. `RuntimeFailed` discards the
in-flight delta and creates an empty-delta failed frame whose parent is the
unchanged step that failed. A failed frame never replaces that step.

Recovery modules may then choose a bounce according to the failure and governance
policy.

Tool failures carry the assistant turn, completed results, failing request, and
remaining requests needed to explain the failure. Manual recovery first reads
`session state`, then rolls back exactly one frame to restore the parent
`RequestExecutor`, and finally supplies the external result with
`session tool-result`. If earlier tools in the same batch succeeded, their
`pending_results` remain in that restored executor.
