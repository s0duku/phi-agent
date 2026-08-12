# Session

`Session` is the CLI and serialization boundary around a step expression. Session
transformations consume ownership and return a new Session, such as append and
rollback.

The evaluator does not operate on Session directly. It consumes the contained
expression and produces an agent snapshot that can be converted back to Session.

Every CLI command that consumes Session state requires an explicit target. A
path selects file-backed state and `-` selects stdin/stdout for transforming or
inspecting state. `session delete` is deliberately file-only because it closes
jobs referenced by that file before removing it. Loading never creates state
implicitly: missing files and empty input are errors, while
`phi session new SESSION` or `phi session new -` are the creation boundaries.
