#![cfg(unix)]

use std::time::Duration;

use crate::headlessterm::{HeadlessTerminal, JobAccess, JobAccessResult, JobHandle, JobStatus};

const EXPIRATION: Duration = Duration::from_secs(5);

#[test]
fn async_trait_preserves_the_complete_job_lifecycle() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handle, initial) = HeadlessTerminal::new()
            .exec_job("printf ready; sleep 10", Duration::from_secs(1), EXPIRATION)
            .await
            .unwrap();
        assert!(initial.is_running());
        assert_eq!(initial.outputs(), "ready");
        let handle = handle.unwrap();

        let read = HeadlessTerminal::new()
            .access_job(
                JobHandle(handle.0.clone()),
                JobAccess::Interact {
                    data: String::new(),
                    return_when: Duration::ZERO.into(),
                },
            )
            .await
            .unwrap();
        let JobAccessResult::Interacted(read) = read else {
            panic!("interact returned write acknowledgment");
        };
        assert!(read.is_running());
        assert!(read.outputs().is_empty());

        let closed = HeadlessTerminal::new().close_job(handle).await.unwrap();
        assert!(matches!(closed.status(), JobStatus::Closed(_)));
    });
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/tests/headlessterm/unix_container_body.inc"
));
