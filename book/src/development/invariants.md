# Design invariants

Keep the following invariants visible while changing the runtime:

- Runtime code operates on expressions, not Sessions.
- Failed runtime frames have an empty delta.
- Runtime modules write persistent variables through the current delta, not by
  mutating a step expression.
- Delta composition applies the base first and the current delta second; later
  variable effects override earlier effects for the same variable.
- Session transformations consume ownership.
- Serialization describes the current types directly.
