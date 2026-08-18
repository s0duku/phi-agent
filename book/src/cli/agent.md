# Agent commands

`phi step` evaluates one boundary, `phi run` evaluates until its normal stop
condition, and `phi yolo` continues through the default recovery path.

Common input operations include `--user` and `--assistant`.

```bash
phi session new work.session
phi step work.session --user "Inspect one boundary"
phi run work.session --user "Continue until the next boundary"
phi yolo work.session --user "Continue through recoverable failures"
phi run work.session --null-executor --user "Answer without built-in tools"
phi run work.session --runner bash --runner-arg=-c --user "Use bash for shell jobs"
```

The Session target is required. Use a file path for persistent state or `-` for
explicit stdin/stdout transport. Start a pipeline with `phi session new -`.
Missing files, empty files, and empty stdin are rejected; agent commands never
infer a new Session or interpret stdin as a user message.

By default, built-in shell jobs use the host shell. `--container NAME` targets
an already-running Docker container. Alternatively, `--runner PROGRAM` passes
each shell command to that program as one final argument, after any repeated
`--runner-arg ARG` values. For example, `--runner bash --runner-arg=-c` executes
commands through `bash -c`. Local shell jobs and custom runner jobs inherit the
caller’s current working directory; Docker jobs keep container cwd semantics.
`--runner` and `--container` are mutually exclusive.
