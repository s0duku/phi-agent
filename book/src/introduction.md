# Introduction

Phi is a Rust CLI agent runtime. It evaluates ReAct steps as functional expressions,
keeps side-effecting tools in runtime-owned state, and exposes Session as the
serialization and command-line boundary.

This book documents the current implementation and design constraints. It is a
development specification, not a stability promise.

## Start here

```bash
cargo run -p phi -- --help
cargo run -p phi -- session --help
```
