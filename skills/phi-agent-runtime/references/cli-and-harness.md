# CLI and Harness Reference

## Transport selection

| Mode | Input | Output | Use |
| --- | --- | --- | --- |
| Pipeline | Session JSON on stdin, or no stdin plus `--user/--assistant` | Updated Session JSON on stdout | CI, pipes, subprocess harnesses |
| File | Existing Session path argument | Same file rewritten | Long-running or inspectable workflows |
| Session subcommand | Session path (or configured transport) | Transformed Session/diagnostic JSON | External orchestration and intervention |

Examples:

```bash
phi step --user 'one boundary'
printf '%s\n' "$session_json" | phi run > updated.json
phi session new work.session
phi run work.session --user 'continue'
phi session peek work.session
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

`append` adds a user or assistant message to the outer delta. `next --provider` creates a provider request frame. `replace --provider` replaces the outer step while retaining its composed delta. `rollback` removes one frame. `tool-result --text TEXT`, `--json JSON`, `--text-file FILE`, or `--json-file FILE` supplies the first pending executor result without invoking an executor. Prefer file input when the result may exceed command-line argument limits.

`session history` emits the committed `PhiHistory` as JSON by default. Use
`session history SESSION --view` for the echo-style transcript view.

Create a file-backed session explicitly with `phi session new PATH`; Phi will
refuse to treat a missing path as an existing session. In pipeline mode, a
non-empty stdin is Session JSON. A new pipeline Session requires `--user` or
`--assistant`.

Check `peek` before intervention. It is a state summary and does not expose tool
call bodies. When it reports `request_executor`, inspect the complete Session's
outer frame for the first pending call, then pass exactly one result through
`tool-result`.

## Robust shell conventions

- Use `set -euo pipefail`; keep a temporary Session path under a trap.
- Reserve stdout for machine-readable Session JSON. Send progress, `peek`, and
  debug data to stderr, and preserve stderr from successful Phi commands.
- Bound loops with an iteration counter or `--max-steps`; stop on explicit turn-end/failure state.
- Quote Session paths and user text. Treat tool output as data, not shell syntax.
- For external tools, inspect the pending call, execute it in the harness's policy boundary, then pass the result through `session tool-result`.
