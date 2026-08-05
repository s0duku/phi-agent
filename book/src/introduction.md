# Introduction

Phi is a CLI-oriented Agent Runtime written in Rust. Its command-line interfaces
directly expose the library's Session transformations, atomic ReAct evaluation,
and HeadlessTerminal job operations. Session is the serialization boundary;
side-effecting tools remain in runtime-owned state.

This book documents the current implementation and design constraints. It is a
development specification, not a stability promise.

## Start here

```bash
cargo run -p phi -- --help
cargo run -p phi -- session --help
```
