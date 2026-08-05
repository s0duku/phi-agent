# StepBounce semantics

The bounce chosen by governance determines how a step result becomes a new frame:

- `CreateNextStep` carries the current delta into a new frame.
- `ReplaceBaseStep` replaces the base step and composes the base delta followed
  by the current delta.
- `RuntimeFailed` creates a failed frame with an empty delta.
- `RollbackStep` removes one frame.
- `KeepBaseStep` preserves the base and skips the current step.

History and persistent variable effects are part of the same delta and follow
these rules together. See [Variable effects](../architecture/variable-effects.md#bounce-semantics).
