# Session

`Session` is the CLI and serialization boundary around a step expression. Session
transformations consume ownership and return a new Session, such as append and
rollback.

The evaluator does not operate on Session directly. It consumes the contained
expression and produces an agent snapshot that can be converted back to Session.
