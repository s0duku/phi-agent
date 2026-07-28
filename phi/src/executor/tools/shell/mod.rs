pub(crate) mod job;

use std::sync::Arc;

use crate::executor::PhiTool;

pub(crate) use job::{ShellJobCloseTool, ShellJobExecTool, ShellJobInteractTool};

pub(crate) fn shell_job_tools() -> Vec<Arc<dyn PhiTool>> {
    vec![
        Arc::new(ShellJobExecTool),
        Arc::new(ShellJobInteractTool),
        Arc::new(ShellJobCloseTool),
    ]
}
