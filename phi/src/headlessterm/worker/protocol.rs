use serde::{Deserialize, Serialize};

use crate::headlessterm::job::{HeadlessTermError, JobAccess};

#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    Access(JobAccess),
    Close,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Written {
        status: ProcessStatus,
    },
    Terminal {
        status: Status,
        output: String,
        truncated: bool,
        waited_ms: u64,
    },
    Failed {
        status: ProcessStatus,
        error: HeadlessTermError,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Status {
    RunningOutputSettled,
    RunningScreenSampled,
    RunningWaitElapsed,
    Exited(i8),
    Closed(i8),
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum ProcessStatus {
    Running,
    Exited(i8),
}
