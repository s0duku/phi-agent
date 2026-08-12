# Observable Session And Recovery Rules

This reference intentionally describes only behavior a CLI harness can observe;
it does not require access to Phi source code.

## Session shape

Session JSON contains a `frames` array ordered from the root to the current
outer frame. The current step is `frames[-1].step`. `session state` explains
that step as stable structured information for orchestration.

Common step kinds are `request_provider`, `request_executor`, `request_compact`,
`compacted`, `turn_end`, and `failed`. A failed step contains an error object;
runtime failures are explicit state, not assistant text.

An assistant message owns its tool-call list. A `request_executor` holds that
assistant turn and the batch's `pending_results`. Each non-final tool result
replaces this executor instead of adding another executor frame. The final
result creates a `request_provider` child frame whose delta commits the
assistant and all tool results to history.

## Tool recovery

`request_executor` means a pending call can be resolved by one
`session tool-result`. A `failed` step cannot be resolved directly. For
`tool_not_found` or a tool failure:

1. Read the failed error and retain its call ID, name, and arguments.
2. Execute or emulate the call outside Phi.
3. Run `session rollback` to restore the pending executor request.
4. Run `session tool-result` with the custom result.
5. Run again and inspect the next boundary.

Rollback is therefore a recovery operation, not a history deletion shortcut.
If a normal executor request has not failed, do not rollback before
`tool-result`.

Every runtime failure is an empty-delta child of the unchanged step that
failed. If the second or later tool fails, rollback restores the same executor
with its earlier `pending_results`; it does not restart the batch. Record the
structured failure before rollback because rollback removes that failed frame.

## Scheduling

`run` is the default throughput-oriented scheduler. Inspect after each run so a
harness can pause for approval, route a tool, recover a failure, or checkpoint.
Use `step` when the policy must act after every atomic transition. Use `yolo`
when built-in continuation and recovery should be allowed without intervention.
