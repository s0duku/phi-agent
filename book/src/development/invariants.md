# Design invariants

Keep the following invariants visible while changing the runtime:

- Runtime code operates on expressions, not Sessions.
- Failed runtime frames have an empty delta.
- Session transformations consume ownership.
- Serialization describes the current types directly.
