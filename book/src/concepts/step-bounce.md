# StepBounce semantics

The bounce chosen by governance determines how a step result becomes a new frame:

- `CreateNextStep` carries the current delta into a new frame.
- `ReplaceBaseStep` replaces the base step and composes the base delta followed
  by the current delta.
- `RuntimeFailed` always creates a failed frame with an empty delta over the
  unchanged base step.
- `RollbackStep` removes one frame.
- `KeepBaseStep` preserves the base and skips the current step.

History and persistent variable effects are part of the same delta and follow
these rules together. See [Variable effects](../architecture/variable-effects.md#bounce-semantics).

Bounce selection completely determines expression structure. Runtime step
evaluation does not directly add, replace, or remove frames outside this
interpreter. In particular, a failure raised while preparing or validating a
replacement still expands over the original base, so one rollback restores it.
