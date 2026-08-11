# Agent commands

`phi step` evaluates one boundary, `phi run` evaluates until its normal stop
condition, and `phi yolo` continues through the default recovery path.

Common input operations include `--user` and `--assistant`.

```bash
phi step --user "Inspect one boundary"
phi run work.session --user "Continue until the next boundary"
phi yolo work.session --user "Continue through recoverable failures"
phi run --null-executor --user "Answer without built-in tools"
phi run --runner bash --runner-arg=-c --user "Use bash for shell jobs"
```

Without a Session path or piped Session JSON, at least one CLI message is
required to create a new Session.

By default, built-in shell jobs use the host shell. `--container NAME` targets
an already-running Docker container. Alternatively, `--runner PROGRAM` passes
each shell command to that program as one final argument, after any repeated
`--runner-arg ARG` values. For example, `--runner bash --runner-arg=-c` executes
commands through `bash -c`. `--runner` and `--container` are mutually exclusive.
