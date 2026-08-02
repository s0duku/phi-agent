# Step expressions

`PhiStepExpr` represents a frame chain. Each frame contains a step and a delta;
history and store values are resolved through the expression chain.

The frame representation keeps the operational state explicit while allowing
cheap cloning through shared ownership.
