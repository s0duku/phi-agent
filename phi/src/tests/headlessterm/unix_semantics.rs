#![cfg(unix)]

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/tests/headlessterm/unix_semantics_body.inc"
));
