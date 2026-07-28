# Development Specification

## PHI

Phi is a CLI agent runtime build with rust. It's development should flow these specfication:

* rust lib crate `./phi` is the main implementation for phi runtime, it should be treated as the source of truth in this workspace
* `./phi` try to follow the principles of functionalization and serialization as much as possible. It's treat  ReAct's different step as different step-expr(S-expression style) and eliminate side-effect as much as possible, the same step-expr should have the same operational semantics. Regarding the evolution of the historical message sequence and the explanation of the ReAct steps, `./phi` mainly organizes them into a functional evaluation process. For operations that may cause side effects, such as the invocation of certain tools, a good design is to integrate them into the Runtime object and separate their storage method from the functionalized ReAct steps information when serializing the Session.
* `./phi` utilize rust type system and compiler constraint to describe the abstraction and relation of different modules, keep the types simple and minimize redundancy as much as possible, and ensure the accuracy of interface permission relationships, let the code express it-self rather than depending on comments. Any developer should respect to these principles. Actively incorporate the design into the type interface declaration, leveraging the type system to establish dependencies and constraints to ensure the design, rather than relying on annotations or verbal descriptions that cannot be checked by the compiler or the type system.
* While it is difficult to rely solely on the type system or compiler to guarantee long-term safety and prevent semantic drift, a well-designed type system can leverage the compiler to constrain the codebase to a state that remains human-auditable.
* `./phi` test cases focus on semantic rightness, when introducing new features, or reconstruct code, tests should focused on semantic rightness, consider boundary case as much as possible, not just check the result but also verify the process.
