# Python Harness Patterns

Python is the recommended language for a complex Phi harness. Invoke the CLI
with `subprocess`, keep state as JSON, and make every policy decision visible in
code.

The target machine only needs Python and an installed `phi` executable. Start
the harness with `shutil.which("phi")` and optionally record `phi --version` and
`phi --help` in its audit log.

## Safe CLI wrapper

```python
from __future__ import annotations
import json
import shutil
import subprocess
import sys

class PhiCommandError(RuntimeError):
    pass

if shutil.which("phi") is None:
    raise PhiCommandError("phi CLI was not found on PATH")

def phi(*args: str, state: dict | None = None) -> dict:
    proc = subprocess.run(
        ["phi", *args],
        input=None if state is None else json.dumps(state),
        text=True, capture_output=True, check=False,
    )
    if proc.stderr:
        sys.stderr.write(proc.stderr)
    if proc.returncode:
        raise PhiCommandError(f"phi {' '.join(args)}: {proc.stderr.strip()}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise PhiCommandError(f"Phi returned invalid JSON: {exc}") from exc
```

Pass argument lists, never shell strings. Forward stderr as above or write it to
an audit log, including for successful commands, and keep stdout exclusively for
Session JSON. For file-backed mode, create a path with `phi session new PATH`,
then pass `PATH` to commands so Phi persists every committed checkpoint. Prefer
that mode when a debugging workflow must remain inspectable after interruption.
Do not add `--quiet` unless the user explicitly asks for quiet execution or the
harness deliberately replaces Phi's diagnostic stream.

## Default `run` controller

```python
def drive(state: dict, *, max_runs: int = 20) -> dict:
    for _ in range(max_runs):
        state = phi("run", "-", state=state)
        report = phi("session", "state", "-", state=state)
        kind = report["state"]
        # Failed is terminal too, but recoverable failures must be handled first.
        if kind == "failed":
            state = recover_failed(state, report)
        elif kind == "turn_end":
            return state
        elif kind == "request_executor":
            call = report["next_tool_call"]
            result = dispatch(call)
            state = phi(
                "session", "tool-result", "-", "--json", json.dumps(result), state=state
            )
        # Other non-terminal boundaries (provider/compact) continue with run.
    raise TimeoutError("Phi run budget exhausted")
```

`run` is the normal throughput path: it performs consecutive evaluations before
returning. Inspecting after each `run` retains orchestration control without
paying Python/CLI overhead for every atomic step. Replace `run` with `step` when
you need to observe every transition.

## External tool routing

```python
TOOLS = {"lookup": lookup, "issue_ticket": issue_ticket}

def dispatch(call: dict) -> dict:
    name = call["name"]
    if name not in TOOLS:
        raise PermissionError(f"tool is not allowlisted: {name}")
    arguments = call.get("arguments", {})
    # Validate schema, authorization, timeout, and output size here.
    try:
        return {"ok": True, "value": TOOLS[name](arguments)}
    except Exception as exc:
        return {"ok": False, "error": str(exc), "tool": name}
```

For an ordinary `request_executor`, call `tool-result` directly. Phi consumes
one pending call; inspect again before handling the next one. The report's
`completed_results` is the already-finished prefix of the assistant's tool-call
list. Phi keeps that prefix in the same executor until the final result creates
the provider step and commits the complete batch to history.

## `tool_not_found` recovery

When `run` returns a failed Session, inspect the complete outer frame. A
`tool_not_found` error contains the original `tool_request`. Route it to a
custom implementation, then restore and resolve the request:

```python
def recover_failed(state: dict, report: dict) -> dict:
    failure = report["failure"]
    if failure.get("kind") != "tool_not_found":
        raise PhiCommandError(f"unrecoverable Phi failure: {failure}")
    call = failure["tool"]["request"]
    result = dispatch(call)  # or a dedicated fallback_tool(call)
    state = phi("session", "rollback", "-", state=state)
    return phi(
        "session", "tool-result", "-", "--json", json.dumps(result), state=state
    )
```

The order matters: `tool-result` requires `request_executor`, while the failed
step is not that state. The failed step is an empty-delta child, so rollback
restores the exact executor, including earlier completed results. Preserve the
call ID and return a bounded JSON value. Record the original error and recovery
before rollback removes the failed frame.

## Approval and workflow orchestration

For approval gates, persist a pending approval record outside transient Python
memory, stop the loop, and resume later with a Session message or tool result.
For retries, count attempts and preserve each failure. For workflows, store a
checkpoint after each `run`, route by observed step kind/error, and never infer
progress from assistant prose.

## HeadlessTerminal

```python
proc = subprocess.run(
    ["phi", "headlessterm", "exec", "--wait-ms", "1000", "--", "sh", "-lc", "printf ready"],
    text=True, capture_output=True, check=True,
)
info = json.loads(proc.stdout)
handle = info.get("handle")
if handle:
    # Preserve output/status; close every handle owned by the harness.
    access = phi("headlessterm", "access", handle, "--wait-ms", "1000")
    phi("headlessterm", "close", handle)
```

Handles are runtime process state, not Session data. Preserve `status`,
`output`, `truncated`, and `waited_ms` in the harness audit record.

## Testing

Use a fake provider or `--null-executor`. Assert each state transition, pending
call ordering, completed-result preservation, rollback/recovery ordering,
terminal state, and history change. In a multi-tool fixture, assert that
non-final results do not add executor frames and the final result creates the
provider frame that commits the complete batch.
Use temporary directories and never run destructive real tools in fixtures.
