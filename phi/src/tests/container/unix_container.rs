#![cfg(unix)]

use std::time::Duration;

use crate::container::job::{JobAccess, JobAccessResult, JobContainer, JobHandle, JobStatus};

const EXPIRATION: Duration = Duration::from_secs(5);

#[test]
fn async_trait_preserves_the_complete_job_lifecycle() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (handle, initial) =
            <crate::container::LocalShellJobContainer as JobContainer>::exec_job(
                "printf ready; sleep 10",
                Duration::from_secs(1),
                EXPIRATION,
            )
            .await
            .unwrap();
        assert!(matches!(initial.status(), JobStatus::Running));
        assert_eq!(initial.outputs(), "ready");
        let handle = handle.unwrap();

        let read = <crate::container::LocalShellJobContainer as JobContainer>::access_job(
            JobHandle(handle.0.clone()),
            JobAccess::Interact {
                data: String::new(),
                try_wait: Duration::ZERO,
            },
        )
        .await
        .unwrap();
        let JobAccessResult::Interacted(read) = read else {
            panic!("interact returned write acknowledgment");
        };
        assert!(matches!(read.status(), JobStatus::Running));
        assert!(read.outputs().is_empty());

        let closed = <crate::container::LocalShellJobContainer as JobContainer>::close_job(handle)
            .await
            .unwrap();
        assert!(matches!(closed.status(), JobStatus::Exited(_)));
    });
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/tests/container/unix_container_body.inc"
));
