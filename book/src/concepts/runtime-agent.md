# Runtime and agent

Runtime owns execution concerns such as providers, executors, tools, and modules.
Its setup is one immutable snapshot of the resolved config, command options, and
Phi Home selected at the CLI or library boundary. Agent is the consuming
evaluation boundary around Runtime.

Session remains outside the runtime evaluation loop except at construction and
checkpoint conversion boundaries.
