# Agent commands

`phi step` evaluates one boundary, `phi run` evaluates until its normal stop
condition, and `phi yolo` continues through the default recovery path.

Common input operations include `--user` and `--assistant`.

```bash
phi step --user "Inspect one boundary"
phi run work.session --user "Continue until the next boundary"
phi yolo work.session --user "Continue through recoverable failures"
phi run --null-executor --user "Answer without built-in tools"
```

Without a Session path or piped Session JSON, at least one CLI message is
required to create a new Session.
