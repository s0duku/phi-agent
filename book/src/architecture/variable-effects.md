# Variable effects

Phi expressions carry persistent module variables as effects, not as a snapshot
of a global store. A frame delta records what happened in that frame:

- `Store(value)` stores a variable value.
- `Remove` removes a variable value and hides values stored by parent frames.
- `Missing` is an internal lookup result meaning that the frame did not affect
  the variable.

`Missing` is not serialized. It is the absence of an entry in the frame's effect
map.

## Typed variables

A module declares a variable by pairing its serialized name with its Rust value
type:

```rust,ignore
const RETRY_STATE: PhiVariable<PhiModelRetryState> =
    PhiVariable::new("phi_model_retry_state");
```

The variable handle is then used for every operation:

```rust,ignore
let state = expr.lookup(RETRY_STATE);
delta.store(RETRY_STATE, next_state);
delta.remove(RETRY_STATE);
```

`PhiVariable<T>` makes the value type part of the interface. A caller cannot use
the retry-state handle to store or retrieve another Rust type. Variable
declarations and domain-specific helpers belong to the module that owns the
variable; `expr.rs` does not know about model retry or other module
state.

Variable names still form a serialized string namespace. Rust cannot prevent two
independent modules from declaring different handles with the same string, so
variable declarations should use stable, module-prefixed names and remain easy
to audit.

## Lookup

`PhiStepExpr::lookup` starts at the outermost frame and walks toward its parents:

1. `Stored(value)` deserializes and returns `Some(value)`.
2. `Removed` returns `None` and stops; it masks every older value.
3. `Missing` continues with the parent frame.
4. Reaching the end of the chain returns `None`.

A stored value that cannot be decoded as the variable's declared type also
returns `None` and does not expose an older value. This can only arise from
invalid serialized input or conflicting variable declarations; normal typed API
use prevents it.

The internal three-state lookup result is private to `expr.rs`. Modules see only
`Option<T>` from `lookup`.

## Current-delta operations

Runtime modules write only to the current `PhiExprDelta`:

- `store` records `Store(value)`.
- `remove` records `Remove`.
- `affects` reports whether the current delta contains either effect for a
  variable.

`affects` is deliberately different from `lookup`. It does not resolve the
expression chain or report whether a value currently exists. Governance uses it
when it must distinguish "this transition did not mention the variable" from
"this transition explicitly removed the variable."

Modules do not mutate a `PhiStepExpr` to write variables. Expression-level
`store` and `remove` helpers exist only for test fixture construction. Runtime
evaluation therefore cannot bypass the current delta.

## Normal form and composition

Within one frame, effects are normalized into one map entry per variable. Only
the last effect for a variable matters:

```text
Store(x, 1); Store(x, 2)  == Store(x, 2)
Store(x, 1); Remove(x)    == Remove(x)
Remove(x); Store(x, 2)    == Store(x, 2)
```

Effects for different variables are independent. The normalized map is therefore
equivalent to the observable result of an ordered effect log, without retaining
shadowed operations.

Delta composition is directional and written `base.then(current)`:

- history from `current` is appended after history from `base`;
- variable effects from `current` override effects for the same variable in
  `base`;
- effects for other variables are retained.

Composition does not cross frame boundaries during ordinary expression
construction. Frames must remain separate because rollback removes exactly one
frame and must restore the variable environment visible at its parent.

## Bounce semantics

A bounce handles history and variable effects together by handling the complete
`PhiExprDelta`:

| Bounce | Delta effect |
| --- | --- |
| `CreateNextStep` | Moves the current delta into a new outer frame. |
| `ReplaceBaseStep` | Replaces the base using `base.delta.then(current_delta)`. |
| `RuntimeFailed` | Discards the current delta and creates an empty-delta failed frame over the unchanged base. |
| `RollbackStep` | Discards the current delta and removes one base frame. |
| `KeepBaseStep` | Discards the current delta and preserves the base unchanged. |

Keeping history and variable effects in the same delta prevents a bounce from
committing one while accidentally discarding the other.

## Serialization

The serialized delta exposes its normalized effects directly:

```json
{
  "effects": {
    "phi_example_count": {
      "kind": "store",
      "value": 3
    },
    "phi_example_obsolete": {
      "kind": "remove"
    }
  }
}
```

Phi serializes the current canonical representation. Historical `store`,
`bindings`, `set`, or `unset` aliases are not accepted.
