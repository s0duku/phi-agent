# Ownership boundaries

The intended monotonic path is:

```text
Session -> Runtime -> Agent -> Session
```

Runtime evaluation works on expressions, not Sessions. This keeps command-line
ownership operations separate from runtime step evaluation.
