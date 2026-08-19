<div align="center">
  <img alt="Phi logo" src="assets/phi-logo.svg" width="96" />
  <p><strong>CLI-oriented Agent Runtime</strong></p>
</div>
<div align="center">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg" />
  <a href="https://github.com/s0duku/phi-agent/actions/workflows/ci.yml">
    <img alt="CI" src="https://img.shields.io/github/actions/workflow/status/s0duku/phi-agent/ci.yml?label=CI" />
  </a>
  <a href="https://github.com/s0duku/phi-agent/releases">
    <img alt="Release" src="https://img.shields.io/github/v/release/s0duku/phi-agent" />
  </a>
  <img alt="Platforms" src="https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-informational" />
</div>

---



![Phi demo 1](assets/demo1.gif) ![Phi demo 2](assets/demo2.gif)

## Recommended User Workflow

Phi is intended to be used together with a coding agent. Install the `phi` CLI,
install the bundled `phi-agent-runtime` skill into Codex, Claude Code, or
OpenCode, configure a model API key, and then ask the coding agent to build a
Phi harness for the task you want to automate.

### 1. Install Phi

Download the release for your platform from
[GitHub Releases](https://github.com/s0duku/phi-agent/releases), put the
`phi` executable on `PATH`, and verify it:

```bash
phi --version
phi --help
```

The harness machine only needs the installed `phi` executable and Python for
complex controllers. It does not need a Rust toolchain or a Phi source
checkout.

### 2. Install the Phi skill

The release archive already contains the skill at
`phi/skills/phi-agent-runtime/`. After extracting the archive, copy that
directory into the skill location supported by your coding agent. You can also
clone this repository when using a development checkout:

```bash
git clone --depth 1 https://github.com/s0duku/phi-agent.git
```

The skill is the directory
[`skills/phi-agent-runtime/`](skills/phi-agent-runtime/). Typical global
locations are:

| Coding agent | Skill directory |
| --- | --- |
| Codex | `~/.codex/skills/phi-agent-runtime/` |
| Claude Code | `~/.claude/skills/phi-agent-runtime/` |
| OpenCode | `~/.config/opencode/skills/phi-agent-runtime/` |

For example, on Unix-like systems:

```bash
cp -R phi-agent/skills/phi-agent-runtime ~/.codex/skills/
# or ~/.claude/skills/ and ~/.config/opencode/skills/
```

Project-local installation is also supported by clients that discover
`.agents/skills/`, `.claude/skills/`, or `.opencode/skills/`; copy the same
`phi-agent-runtime` directory there when you want the skill scoped to one
project. Restart the coding agent after installing and verify that it lists
`phi-agent-runtime`.

### 3. Configure a model

Provide the API key through the environment or the Phi Home configuration. Do
not put keys in a Session file, harness source, or git repository:

```bash
export PHI_PROVIDER=openai_chat
export PHI_MODEL=gpt-5
export PHI_KEY='your_api_key'
```

Use the provider and model appropriate for your account. `phi doctor` reports
the resolved configuration without running an agent task:

```bash
phi doctor
```

### 4. Ask the coding agent to build the harness

Describe the automation outcome, available external systems, approval rules,
and how success should be detected. Explicitly ask the coding agent to use the
`phi-agent-runtime` skill and to produce a Python harness around the Phi CLI:

```text
Use $phi-agent-runtime to build a Python harness with Phi for this task:

Monitor our issue queue, summarize new incidents, ask for approval before
creating a ticket, and persist progress so the workflow can resume tomorrow.
Use `run` for normal progress, inspect the Session after each run, route
external tools from Python, recover missing tools with rollback plus
session tool-result, and add a deterministic dry-run test.
```

The coding agent should create the controller, define its tool allowlist and
failure policy, use `phi session state` for state decisions, and test the
workflow with a bounded run budget. For a single simple task, the same skill
can produce a shell harness; Python is recommended once the workflow has
external tools, retries, approvals, or durable checkpoints.

## Phi Runtime

**Phi is a CLI-oriented Agent Runtime written in Rust.** The CLI is its primary
operational interface, rather than a thin wrapper around a hidden interactive UI
or service. Sessions, agent evaluation, recovery, and persistent terminal jobs
are exposed as commands with explicit, serializable inputs and outputs.

This makes the runtime usable as a standalone agent and as a composable command
in scripts, pipelines, CI jobs, and other programs:

```text
config + command + PhiHome -> runtime setup
Session -> one agent step -> Session
terminal job request -> typed job status and output
```

The Rust library in [`phi/`](phi/) defines these semantics. The CLI delegates to
the same library operations, so command-line composition does not introduce a
second execution model.

## Core Interfaces

### Session as the CLI boundary

A `Session` is Phi's durable, serializable state and the boundary between the
CLI and the agent runtime. Session commands are explicit ownership-consuming
transformations: they read one Session, apply one operation, and write the new
Session either to the same file or to stdout.

- `session new` creates initialized state.
- `session append` adds user or assistant messages to the current outer delta;
  use `--user-file` or `--assistant-file` for file-backed input.
- `session store` and `session remove` persist or mask external key/value state
  in the current outer delta.
- `session next` and `session replace` reproduce the corresponding step-frame
  transitions without running the agent.
- `session tool-result` resolves the current executor request with an externally
  supplied result and applies the configured executor tool-output truncation.
  Pass `--no-truncate` only for an already-bounded result that must remain
  unchanged.
- `session state`, `history`, `rollback`, and `delete` inspect or manage state;
  `rollback --to STEP` can retain the nearest frame of a selected step kind.

This interface lets a shell script, a human, or another program inspect and
modify a Session between agent steps without bypassing Session semantics.

### Atomic agent evaluation

Phi treats agent execution as step-by-step evaluation of serialized ReAct
expressions.

- `step` advances a Session by exactly one atomic agent step.
- `run` repeatedly applies that same step operation until the next run boundary.
- `yolo` repeatedly applies it through Phi's default continuation and recovery
  policy.

Failures and context compaction remain explicit step state. Each completed CLI
operation persists the resulting Session, so evaluation can be inspected,
rolled back, resumed, or driven by a different scheduler.

Tool calls from one assistant response form one executor batch. Successful
calls within that batch replace the current `RequestExecutor` step while
accumulating `pending_results`, so they do not create one frame per tool. When
the batch completes, Phi creates a new `RequestProvider` frame whose delta
commits the assistant message and every tool result. A runtime failure always
creates an empty-delta `Failed` frame over the step that failed; rollback
therefore restores the exact executor progress immediately before the failure.

### One HeadlessTerminal job interface

Persistent shell and REPL processes use the same HeadlessTerminal job API at
every boundary. The `phi headlessterm exec`, `access`, and `close` commands call
the Rust `HeadlessTerminal::exec_job`, `access_job`, and `close_job` operations
directly. Their JSON outputs preserve the library return shapes:
`(Option<JobHandle>, JobInfo)`, `JobAccessResult`, and `JobInfo`, respectively.

The built-in `bash_job`, `job_interact`, and `job_close` tools also use this API.
Their tool-result envelope preserves the same handle, output, truncation, wait,
and job-status semantics, including the distinction between exited jobs,
settled output, screen samples, and elapsed waits. CLI users and agents therefore
observe the same terminal lifecycle instead of separate terminal abstractions.

## Documentation

The full architecture, runtime semantics, CLI workflows, private protocols, and
development invariants are maintained in the [Phi Book](book/src/introduction.md).

## Agent Skill

This repository distributes a Codex-compatible skill for teaching other agents
to use Phi and build complex Python or shell harnesses. It covers external-tool
routing, workflows, Session persistence, HeadlessTerminal jobs, and the runtime
semantics needed to preserve correctness. See
[`skills/phi-agent-runtime/`](skills/phi-agent-runtime/) for the skill and its
progressively loaded references.

An agent can load that directory from GitHub and invoke it as `$phi-agent-runtime`.
The distributed skill assumes only an installed `phi` executable on the user's
machine; the book is optional maintainer documentation, while the skill is the
standalone CLI integration and harness guide.

## Why CLI-oriented

- **Composable state:** Session JSON can cross process boundaries through
  stdin/stdout or remain in a file for repeated commands.
- **Auditable execution:** one atomic `step` is the common evaluation unit behind
  the higher-level schedulers.
- **Persistent terminals:** jobs can survive individual tool calls and support
  GDB, shells, and other interactive processes.
- **Selectable command execution:** Phi can use the host shell, enter an
  already-running container, or pass commands through a custom runner program.
  Local shell and custom runner jobs inherit the caller's current working
  directory; Docker jobs keep container cwd semantics.
- **Structural rollback:** the S-expression-style step structure makes rollback
  an explicit Session transformation.

For example, an existing container can be used as the command environment:

```bash
docker run -dit --name phi-test-run docker.io/library/alpine /bin/sh
phi yolo work.session --user "list files" --container phi-test-run
```

A custom runner receives its fixed arguments followed by the complete command
as one final argument. Agent commands apply the same runner to the built-in
`bash_job` tool, and the runner inherits the caller's current working
directory:

```bash
phi yolo work.session --runner bash --runner-arg=-c --user "list files"
```

`--runner` and `--container` are mutually exclusive. Repeat `--runner-arg` for
programs that need more than one fixed argument.


## Config

Example OpenAI-compatible setup:

```bash
export PHI_PROVIDER=openai_chat
export PHI_MODEL=gpt-5
export PHI_KEY=your_api_key
```

Or use `~/.phi/config.yml` with the typed YAML schema:

```yaml
model:
  name: gpt-5
provider:
  kind: openai_chat
  api_key: your_api_key
runtime:
  # Omit this field to use Phi's built-in prompt.
  # Set it to "" to commit an explicitly empty system message.
  system: ""
```

Use `--config FILE` to replace the config location supplied by Phi Home for a
command. `PHI_*` environment variables are applied last and therefore override
either YAML source.

## Sessions

Phi always requires an explicit session target. State-transforming commands use
`-` to select stdin/stdout as the session transport; `session delete` is a
file-management operation and accepts only a file path.

```bash
phi session new - |
  phi run - --user "Hello" |
  phi step -
```

In pipeline mode:

- `-` explicitly selects stdin/stdout
- stdin is parsed as session JSON
- stdout emits the updated session JSON
- `phi session new -` creates the initial Session
- empty stdin is rejected instead of creating an implicit Session

Passing a file path selects file-backed session mode:

```bash
phi session new work.session
phi run work.session --user "Hello"
phi run work.session --user "follow up"
```

In file-backed mode:

- the file must already exist; create it explicitly with `phi session new SESSION`
- an empty file is rejected instead of being treated as a new Session
- appended input and every committed Agent step are atomically written back to the same file
- readers see either the previous complete Session or the new complete Session
- user and assistant input must be passed through explicit message arguments

## Commands

Main commands:

- `phi step SESSION`
- `phi run SESSION`
- `phi yolo SESSION`
- `phi session state SESSION`
- `phi session next SESSION --provider`
- `phi session replace SESSION --provider`
- `phi session tool-result SESSION (--json JSON|--text TEXT|--json-file FILE|--text-file FILE)`
- `phi session store SESSION --key KEY (--json JSON|--json-file FILE|--text TEXT|--text-file FILE)`
- `phi session remove SESSION --key KEY`
- `phi session rollback SESSION [--to STEP]`
- `phi session append SESSION (--user TEXT|--assistant TEXT|--user-file FILE|--assistant-file FILE)`
- `phi headlessterm exec|access|close`
- `phi doctor`
- `phi session new SESSION`
- `phi session history SESSION [--view|--last]`
- `phi session delete SESSION`
- `phi home new|pack|unpack`

Examples:

```bash
phi session new work.session
phi step work.session --user "Inspect the repository"
phi run work.session --quiet --user "Summarize the bug"
phi yolo work.session --user "Keep going until done"
phi session state work.session
phi session next work.session --provider
phi session replace work.session --provider
# When state reports `request_executor`, resolve its next call externally:
phi session tool-result work.session --text "external tool output"
phi session tool-result work.session --json-file large-result.json
phi session rollback work.session
phi doctor
phi session history work.session
# Human-readable echo-style transcript:
phi session history work.session --view
phi session history work.session --last
```

For explicit stdio composition, use `-` as the session target:

```bash
phi session append - --user "piped" < work.session |
  phi session rollback - |
  phi session state -
```

HeadlessTerminal commands expose the library job API as JSON:

```bash
phi headlessterm exec --wait-ms 1000 -- sh -lc 'printf ready'
phi headlessterm exec --runner bash --runner-arg=-c -- 'printf runner-ready'
phi headlessterm access JOB_HANDLE --wait-ms 1000
phi headlessterm access JOB_HANDLE --data 'continue' --write-only
phi headlessterm close JOB_HANDLE
```

## Home

Phi mounts a concrete `PhiHome` before building the runtime.

Home resolution:

- `--home PATH` wins if provided
- a directory path is mounted as a local home
- a sqlite home file such as `.phihome` is mounted directly
- without `--home`, Phi checks `cwd/.phi` first, then falls back to the user home location

Phi can manage both directory homes and packed sqlite homes:

```bash
phi home new .phi
phi home pack .phi -o my.phihome
phi home unpack my.phihome -o unpacked.phi
phi --home my.phihome doctor
```

## Compact And Recovery

Phi includes built-in governance modules for:

- step budgets
- tool execution limits
- model retry boundaries
- automatic context compaction

Auto-compact triggers when rendered provider-visible history approaches the configured context limit. Compact is still a normal step transition, so failures remain visible in session state and can be resumed by later scheduler steps.

Provider failures that stop model output at the configured token limit or contain malformed tool-call JSON are serialized as distinct `ModelOutputLimit` and `ModelToolParseError` failure kinds; both currently follow the normal failed-step fallback.

## Tools

Builtin tool execution is part of the same runtime step model. An assistant
message contains its tool calls directly. The current `RequestExecutor` keeps
the assistant and completed `pending_results`; neither is committed to history
until the complete tool batch produces the next `RequestProvider` frame.

## Notes

- `run` stops at the next boundary.
- `yolo` continues through Phi's default failed-session recovery path.
- `session state` explains the current serialized eval state as structured JSON without building a runtime.
- `doctor` reports initialized runtime status, system prompt, home, and exposed tools.

## Local Build

Workspace aliases are defined in [.cargo/config.toml](.cargo/config.toml).

Build examples:

```bash
cargo build-linux-x64-static
cargo test-linux-x64-static

cargo build-windows-x64-static
cargo test-windows-x64-static

cargo build-macos-arm64-release
cargo test-macos-arm64-release
```

Install helpers currently install the workspace binary locally:

```bash
cargo install-linux-x64-static
cargo install-windows-x64-static
cargo install-macos-arm64-release
```

Or through `xtask` directly:

```bash
cargo run -p xtask -- install --target x86_64-unknown-linux-musl --offline
```

## Release Packaging

The CI workflow builds Linux, Windows, and macOS artifacts.

Current packaged release archives are named after the repository, while their contents remain product-focused under `phi/`, including:

- the `phi` binary
- the workspace `README.md`
- `assets/phi-logo.svg`
- `skills/phi-agent-runtime/`
- bundled `.phi/` content when present in the repository

Archives are published as:

- `phi-agent-linux-x64.tar.gz`
- `phi-agent-windows-x64.zip`
- `phi-agent-macos-arm64.tar.gz`

Recommended release flow:

- regular `push` and `pull_request` runs only build and test
- pushing a version tag like `v0.1.0` builds artifacts and publishes them to this repository's GitHub Release
- if you need to republish an existing tag manually, run `workflow_dispatch` and fill in `release_tag`
