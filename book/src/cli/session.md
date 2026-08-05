# Session commands

Session operations are ownership-consuming transformations:

- `phi session new` creates a root session.
- `phi session append` adds messages to the outer delta.
- `phi session next --provider` adds a request-provider frame.
- `phi session replace --provider` replaces the outer step and preserves its delta.
- `phi session tool-result --json/--text` resolves the first call in the current
  RequestExecutor step without invoking an executor.
- `phi session peek` reports the current evaluation state as JSON.
- `phi session rollback` removes one frame.

```bash
phi session new work.session
phi session append work.session --user "Additional context"
phi session peek work.session
phi session next work.session --provider
phi session rollback work.session
```

`tool-result` is valid only when `peek` reports that the current frame is
`RequestExecutor`. It consumes the first pending call and derives its call ID
from the Session:

```bash
phi session tool-result work.session --text "external tool output"
phi session tool-result work.session --json '{"status":"ok"}'
```
