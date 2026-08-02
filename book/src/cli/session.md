# Session commands

Session operations are ownership-consuming transformations:

- `phi session new` creates a root session.
- `phi session append` adds messages to the outer delta.
- `phi session next --provider` adds a request-provider frame.
- `phi session replace --provider` replaces the outer step and preserves its delta.
- `phi session peek` reports the current evaluation state as JSON.
- `phi session rollback` removes one frame.
