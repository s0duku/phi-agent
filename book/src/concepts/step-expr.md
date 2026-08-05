# Step expressions

`PhiStepExpr` represents a frame chain. Each frame contains a step and a delta;
history and persistent module variables are resolved through the expression
chain.

The frame representation keeps the operational state explicit while allowing
cheap cloning through shared ownership.

Persistent variables are represented as normalized frame effects rather than a
store snapshot. See [Variable effects](../architecture/variable-effects.md).
