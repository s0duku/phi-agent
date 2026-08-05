# Headless terminal

Headless terminal commands manage persistent shell jobs and terminal interactions.
The returned status distinguishes exited jobs, settled output, sampled screens, and
wait expiry.

The CLI uses the same request and response shapes as the Rust job API:

```bash
phi headlessterm exec --wait-ms 1000 -- sh -lc 'printf ready'
phi headlessterm access JOB_HANDLE --wait-ms 1000
phi headlessterm access JOB_HANDLE --data 'continue' --write-only
phi headlessterm close JOB_HANDLE
```
