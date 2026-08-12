# Design invariants

Keep the following invariants visible while changing the runtime:

- Runtime code operates on expressions, not Sessions.
- Runtime expression changes are selected by `StepBounce`; each bounce has one
  fixed frame transformation.
- Failed runtime frames have an empty delta and preserve the failed base as
  their parent.
- A tool batch replaces its `RequestExecutor` while pending and creates a new
  `RequestProvider` frame only when every tool result is available.
- Runtime modules write persistent variables through the current delta, not by
  mutating a step expression.
- Delta composition applies the base first and the current delta second; later
  variable effects override earlier effects for the same variable.
- Session transformations consume ownership.
- Serialization describes the current types directly.
