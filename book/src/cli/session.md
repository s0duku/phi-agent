# Session commands

Session operations are ownership-consuming transformations:

- `phi session new` creates a root session.
- `phi session append` adds messages to the outer delta.
- `phi session next --provider` adds a request-provider frame.
- `phi session replace --provider` replaces the outer step and preserves its delta.
- `phi session tool-result --json/--text/--json-file/--text-file` resolves the
  first call in the current RequestExecutor step without invoking an executor.
- `phi session state` explains the current evaluation state as structured JSON.
- `phi session rollback` removes one frame.
- `phi session delete` closes referenced jobs and deletes a Session file. It is
  file-only and does not accept stdin/stdout transport.

`phi session history` prints the committed `PhiHistory` as JSON, suitable for
machine-readable pipelines. Pass `--view` to render the echo-style transcript.

```bash
phi session history work.session
phi session history work.session --view
```

```bash
phi session new work.session
phi session append work.session --user "Additional context"
phi session state work.session
phi session next work.session --provider
phi session rollback work.session
```

`phi step`, `phi run`, and `phi yolo` atomically replace a file-backed Session
after appending input and after every committed Agent step. A concurrent reader
therefore observes a complete previous or current checkpoint, never a partially
written Session. Pipeline mode continues to emit only the final Session JSON.

`tool-result` is valid only when `state` reports that the current frame is
`RequestExecutor`. It consumes the first pending call and derives its call ID
from the Session:

```bash
phi session tool-result work.session --text "external tool output"
phi session tool-result work.session --json '{"status":"ok"}'
phi session tool-result work.session --json-file large-result.json
phi session tool-result work.session --text-file large-output.txt
```

Within a multi-tool batch, each non-final result replaces the current
`RequestExecutor` and appends to its `pending_results`; no executor frame is
added. The final result creates a new `RequestProvider` frame whose delta
contains the pending messages, the assistant with its complete tool-call list,
and every tool result. Runtime tool failures instead create a `Failed` child
frame with an empty delta. Run `state` before recovery, then `rollback` to
restore the executor and `tool-result` to inject an external result.
