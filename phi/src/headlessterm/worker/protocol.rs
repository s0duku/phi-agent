use serde::{Deserialize, Serialize};

use crate::headlessterm::job::{HeadlessTermError, JobAccess, ReturnWhen};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Request {
    Access(JobAccess),
    Interact {
        request_id: u64,
        data: String,
        return_when: ReturnWhen,
    },
    Cancel {
        request_id: u64,
    },
    Acknowledge {
        request_id: u64,
    },
    Close,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
