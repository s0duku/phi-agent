<div align="center">
  <img alt="Phi logo" src="assets/phi-logo.svg" width="96" />
  <p><strong>A pure CLI agent for persistent interactive terminal workflows</strong></p>
</div>
<div align="center">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue.svg" />
  <a href="https://github.com/s0duku/phi-agent/actions/workflows/ci.yml">
    <img alt="CI" src="https://img.shields.io/github/actions/workflow/status/s0duku/phi-agent/ci.yml?label=CI" />
  </a>
</div>

---



![Phi demo 1](assets/demo1.gif) ![Phi demo 2](assets/demo2.gif)

## Phi Agent

**Phi** is a cli agent without heavy TUI, simply take everyhing from command arguments and enviroment variables, work with session through **stdin** and **stdout**.

Phi reads input from command arguments, environment variables, `PhiHome`, and optional session JSON on stdin, then writes the updated session back to stdout or a session file.

Phi treats agent execution as step-by-step session evaluation.

- `step` advances a session by exactly one atomic agent step.
- `run` keeps stepping until the session reaches its next boundary.
- `yolo` keeps stepping more aggressively through Phi's default recovery path.

## Documentation

The full architecture, runtime semantics, CLI workflows, private protocols, and
development invariants are maintained in the [Phi Book](book/src/introduction.md).

## Why Phi

* **Headless Terminal**, `phi` can run command with headless terminal, so it can use GDB and other REPL command.
* **Outside Cointainer**, instead of put `phi` into a container, `phi` takes advantage of headless terminal, use container's enviroment directly. 
  * `docker run -dit --name phi-test-run docker.io/library/alpine /bin/sh`
  * `phi yolo --user "list file" --container phi-test-run`
* **Free Rollback**, `phi` use s-expression style to store the history, which allow easy to rollback.


## Config

Example OpenAI-compatible setup:

```bash
export PHI_PROVIDER=openai_chat
export PHI_MODEL=gpt-5
export PHI_KEY=your_api_key
export PHI_SYSTEM=""
```

Or you can use `~/.phi/config.toml` to store this config

## Sessions

Without an explicit session path, Phi uses stdin/stdout as a session transport:

```bash
phi run --user "Hello"
cat session.json | phi step
```

In pipeline mode:

- empty stdin means "start from a new empty session"
- non-empty stdin is parsed as session JSON
- stdout emits the updated session JSON

If you pass `[SESSION]`, Phi switches to file-backed session mode:

```bash
phi session new work.session
phi run work.session --user "Hello"
echo "follow up" | phi run work.session
```

In file-backed mode:

- the file must already exist; create it explicitly with `phi session new SESSION`
- the updated session is written back to the same file
- stdin is treated as plain user text, not session JSON

## Commands

Main commands:

- `phi step [SESSION]`
- `phi run [SESSION]`
- `phi yolo [SESSION]`
- `phi session peek [SESSION]`
- `phi doctor`
- `phi session new [SESSION]`
- `phi session history [SESSION]`
- `phi home new|pack|unpack`

Examples:

```bash
phi step --user "Inspect the repository"
phi run --quiet --user "Summarize the bug"
phi yolo work.session --user "Keep going until done"
phi session peek work.session
phi doctor
phi session new work.session
phi session history work.session
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
- loop guard
- automatic context compaction

Auto-compact triggers when rendered provider-visible history approaches the configured context limit. Compact is still a normal step transition, so failures remain visible in session state and can be resumed by later scheduler steps.

## Tools And Plugins

Builtin tool execution is part of the same runtime step model. Tool calls are staged in the session step state first and only committed to history after execution completes.

Python plugin support is subprocess-based. Phi does not link Python at build time; instead it probes a local Python executable at runtime and keeps a worker process alive for plugin loading and tool execution.

You can inspect plugin/runtime status with:

```bash
phi doctor
```

If discovery is not enough:

```bash
export PHI_PYTHON=/usr/bin/python3
phi doctor
```

## Notes

- `run` stops at the next boundary.
- `yolo` continues through Phi's default failed-session recovery path.
- `session peek` reports the current session eval state and governance status as JSON.
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
- bundled `.phi/` content when present in the repository

Archives are published as:

- `phi-agent-linux-x64.tar.gz`
- `phi-agent-windows-x64.zip`
- `phi-agent-macos-arm64.tar.gz`

Recommended release flow:

- regular `push` and `pull_request` runs only build and test
- pushing a version tag like `v0.1.0` builds artifacts and publishes them to this repository's GitHub Release
- if you need to republish an existing tag manually, run `workflow_dispatch` and fill in `release_tag`
