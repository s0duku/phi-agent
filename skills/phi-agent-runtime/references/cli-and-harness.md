# CLI and Harness Reference

## Transport selection

| Mode | Input | Output | Use |
| --- | --- | --- | --- |
| Pipeline | `-` target plus Session JSON on stdin | Updated Session JSON on stdout | CI, pipes, subprocess harnesses |
| File | Existing Session path argument | Same file rewritten | Long-running or inspectable workflows |
| Session subcommand | Session path or explicit `-` | Transformed Session/diagnostic JSON | External orchestration and intervention |

Examples:

```bash
phi session new - | phi step - --user 'one boundary'
printf '%s\n' "$session_json" | phi run - > updated.json
phi session new work.session
phi run work.session --user 'continue'
phi session state work.session
```

Use `--null-executor` to test model-only behavior. Use `--container NAME` when
the executor should target an already-running container. Use `--runner PROGRAM`
with repeated `--runner-arg ARG` values for a custom command carrier; Phi appends
the complete shell command as one final argument. For example,
`--runner bash --runner-arg=-c` produces `bash -c COMMAND`. Runner and container
selection are mutually exclusive, and Agent commands apply the selected target
to the built-in `bash_job` tool. Run `phi doctor` to inspect resolved
home/configuration.

Configuration can be selected with `--home` or `--config`; `PHI_*` environment
variables override YAML configuration. Use `phi --help` and subcommand help as
the authority for flags available in the installed version.

An independently distributed harness should not assume a repository checkout,
Cargo, or a particular Phi version. Probe capabilities at startup and report
the exact command and stderr when a required subcommand is unavailable.

During harness development and debugging, preserve Phi's stderr as an audit
log, or use a persistent Session file so each checkpoint remains inspectable.
Do not make `--quiet` the default. Add it only when the user explicitly requests
quiet output or the harness provides an intentional replacement log stream.

## Session operations

`append` adds a user or assistant message to the outer delta. `store --key KEY --json JSON` (or `--json-file`, `--text`, `--text-file`) persists an external value in the outer delta; `remove --key KEY` writes a removal tombstone. `next --provider` creates a provider request frame. `replace --provider` replaces the outer step while retaining its composed delta. `rollback` removes one frame, while `rollback --to STEP` removes frames until the nearest requested kind is current. `tool-result --text TEXT`, `--json JSON`, `--text-file FILE`, or `--json-file FILE` supplies the first pending executor result without invoking an executor and applies the configured executor output sanitizer. Use `--no-sanitize` only when the external result is already bounded and must be preserved byte-for-byte. Prefer file input when the result may exceed command-line argument limits; sanitization still applies after file input unless disabled.

`session history` emits the committed `PhiHistory` as JSON by default. Use
`session history SESSION --view` for the echo-style transcript view, or
`session history SESSION --last` to emit the last committed message as JSON
(`null` for an empty history).

Inline message flags also have file-backed forms: `--user-file FILE` and
`--assistant-file FILE`. They are available on `session append`, `run`, `yolo`,
and `step`, and retain their ordering relative to inline message flags.

Create a file-backed session explicitly with `phi session new PATH`; Phi will
refuse to treat a missing path as an existing session. In pipeline mode, `-`
explicitly selects stdin/stdout and stdin is Session JSON. Start a new pipeline
Session with `phi session new -`. Empty stdin and empty files are errors, and
stdin is never inferred to be a user message.

Local shell jobs and custom runner jobs inherit the caller's current working
directory. Docker jobs keep the container's cwd semantics, so do not assume the
host cwd exists inside the container.

Check `state` before intervention. It explains the current step as structured
JSON, including the next pending tool call when the state is
`request_executor`; then pass exactly one result through `tool-result`.

For multiple calls, inspect `completed_results`, `next_tool_call`, and
`remaining_tool_calls`. A non-final result updates the same executor state. The
final result creates a provider frame and commits the pending assistant and all
results together. If execution failed, record the failure state, rollback once
to restore that executor progress, and then inject the external result.

## Robust shell conventions

- Use `set -euo pipefail`; keep a temporary Session path under a trap.
- Reserve stdout for machine-readable Session JSON. Send progress, `state`, and
  debug data to stderr, and preserve stderr from successful Phi commands.
- Bound loops with an iteration counter or `--max-steps`; stop on explicit turn-end/failure state.
- Quote Session paths and user text. Treat tool output as data, not shell syntax.
- For external tools, inspect the pending call, execute it in the harness's policy boundary, then pass the result through `session tool-result`.
