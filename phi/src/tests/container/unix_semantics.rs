#![cfg(unix)]

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/tests/container/unix_semantics_body.inc"
));
