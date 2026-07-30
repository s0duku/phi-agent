use serde::{Deserialize, Serialize};

use crate::headlessterm::job::JobAccess;

#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    Access(JobAccess),
    Close,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Written {
        status: Status,
    },
    Terminal {
        status: Status,
        output: String,
        truncated: bool,
        waited_ms: u64,
    },
    Failed {
        status: Status,
        error: String,
    },
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum Status {
    Running,
    Exited(i8),
}
