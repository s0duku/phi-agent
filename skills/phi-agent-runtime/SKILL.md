---
name: phi-agent-runtime
description: Teach an Agent to use Phi and build Python-first harnesses around its CLI. Use for Session workflows, `run`/`step` orchestration, external tool routing, tool_not_found recovery, approvals, retries, CI automation, persistent terminal jobs, and durable Agent workflows.
---

# Using Phi To Build Harnesses

Use this skill as the practical manual for an Agent controlling Phi. The only
runtime dependency is an installed `phi` executable; the user does not need the
Phi source tree, Rust toolchain, or documentation repository. The goal is to
turn a user's task into a small controller around the `phi` CLI. Prefer
Python `subprocess` wrappers for complex harnesses so routing, policy, retries,
approvals, persistence, and audit logs stay explicit.

When a user asks for a harness: identify the task input, the desired completion
condition, the external actions Phi may request, and what should happen after
each `run`; then implement that policy around Session JSON. Do not start by
reimplementing an Agent loop or by editing Phi internals.

## Start from the installed CLI

Before building a harness, run `phi --help`, `phi session --help`, and the help
for any capability the task needs. Treat the installed CLI's help and successful
JSON output as the version-specific contract. Fail early with a useful message
if `phi` is missing from `PATH`.

For a new task, initialize state with either:

```bash
phi step --user "the user's task"
```

or a durable file:

```bash
phi session new task.session
phi session append task.session --user "the user's task"
```

The Python harness may use pipeline JSON or a file path; choose one transport
for the whole controller.

## The operating contract

- Treat `Session` as the only durable Agent state. Commands read a Session and
  produce the next Session; persist every returned JSON or file update.
- Prefer `run` for normal progress: it performs consecutive atomic evaluations
  efficiently and returns at a useful boundary. Inspect the resulting Session
  after every `run` and decide what the harness should do next.
- Use `step` when the harness must intervene between individual evaluations,
  debug a transition, or implement a very fine-grained scheduler. Use `yolo`
  only when Phi's built-in continuation/recovery policy is desired wholesale.
- Keep machine-readable Session JSON on stdout and diagnostics on stderr.
- Keep harness runs observable while developing or debugging: forward or record
  Phi's stderr as logs, or use a persistent Session file as a durable trace.
  Do not add `--quiet` by default; use it only when the user explicitly requests
  quiet execution or the harness deliberately replaces Phi's diagnostics with
  equivalent logging.
- Never infer workflow state from assistant prose. Inspect `phi session peek`
  and the current Session frame.

## Normal run-and-inspect loop

```text
create or load Session
  -> append user/workflow input
  -> run (bounded by max steps)
  -> peek the resulting state
  -> terminal: finish
  -> request_executor: execute/approve a pending call, then tool-result
  -> failed: classify and recover, rollback, or stop
  -> repeat run
```

`peek` is a summary containing `step.kind`, `detail`, `is_terminal`, history
count, and module probes. When it reports `request_executor`, read the complete
Session JSON's outer frame (`frames[-1].step.tool_calls[0]`) to obtain the first
pending call. `tool-result` consumes one call; multiple calls are handled one at
a time across subsequent inspections.

## Recover `tool_not_found` with a custom tool

Phi can end a `run` in a failed state when the requested tool is unavailable.
The failed step is not itself a `request_executor`, so do not call
`tool-result` immediately. A harness can provide the missing capability with
this sequence:

```text
run -> peek sees step.kind == failed
  -> inspect Session frames[-1].step.error.kind == tool_not_found
  -> execute the call in the harness (or route it to a custom service)
  -> session rollback       # restore the original pending executor request
  -> session tool-result    # inject the custom JSON/text result
  -> run again               # let Phi continue with the result in history
```

Preserve the original call ID and tool name. Validate arguments and apply the
harness's authorization, timeout, and output-size policy before side effects.
If the call cannot be fulfilled, stop or inject a structured error result; do
not retry forever. For ordinary `request_executor` states, skip rollback and
call `tool-result` directly.

## CLI capability map

1. Use `phi session new|append|peek|history|rollback` for durable state edits.
   `session history` emits the committed `PhiHistory` as JSON by default; add
   `--view` only when an echo-style human-readable transcript is needed.
   Add `next --provider` or `replace --provider` when a workflow must create or
   replace a provider boundary without evaluating it.
2. Use `phi run [SESSION]` for the main workflow scheduler. Set `--max-steps`
   as the per-run budget and inspect the returned Session before continuing.
3. Use `phi step [SESSION]` for per-transition inspection or custom scheduling.
4. Use `phi session tool-result [SESSION] --json JSON|--text TEXT` to resolve
   exactly one pending tool call without invoking Phi's executor. Use
   `--json-file FILE` or `--text-file FILE` when the result may exceed command-line
   argument limits.
5. Use `phi headlessterm exec|access|close` for persistent shell, REPL,
   debugger, or server jobs. Keep its handle outside Session and close it. Use
   `--runner PROGRAM` with repeated `--runner-arg ARG` values when commands must
   pass through a custom carrier.
6. Use `phi doctor` to inspect resolved home, configuration, system prompt, and
   exposed tools. Use `--null-executor` for model-only deterministic tests.

Read [references/python-harness.md](references/python-harness.md) for the
recommended wrapper and recovery controller. Read
[references/cli-and-harness.md](references/cli-and-harness.md) for transport and
command details. Read [references/semantics.md](references/semantics.md) for
externally observable Session and recovery rules.

## Harness design checklist

- Bound every `run` with a step budget and record each `peek` result.
- Preserve successful-command stderr in the harness log, not only stderr from
  failed subprocesses. Prefer a persistent Session file for workflows that need
  post-failure inspection or transition-by-transition debugging.
- Serialize checkpoints before dispatching external work.
- Allowlist tool names and validate JSON arguments.
- Make approval, retry, timeout, rollback, and failure policies explicit.
- Never concurrently write one Session; parallelize independent Sessions.
- Return non-zero from CI when Phi exits unsuccessfully, JSON is malformed, a
  failed state is unrecoverable, or an expected terminal condition is absent.
- Test the process (state kinds, pending calls, history, recovery ordering), not
  only final assistant text.
