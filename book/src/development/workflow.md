# Development workflow

The `phi` crate is the source of truth. Prefer small changes that preserve the
functional step model and use existing local abstractions.

Before submitting a change:

```bash
cargo fmt --all -- --check
cargo test -p phi
```
