# Runtime and agent

Runtime owns execution concerns such as providers, executors, tools, modules, and
home configuration. Agent is the consuming evaluation boundary around Runtime.

Session remains outside the runtime evaluation loop except at construction and
checkpoint conversion boundaries.
