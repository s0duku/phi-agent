# Failure and recovery

Failures are represented as explicit failed steps. Runtime failures discard the
in-flight delta so a failed frame cannot accidentally commit partial work.

Recovery modules may then choose a bounce according to the failure and governance
policy.
