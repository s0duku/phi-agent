#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerLaunchReport {
    Ready { handle: String },
    Failed { error: HeadlessTermError },
}

impl WorkerLaunchReport {
    pub(crate) fn ready(handle: impl Into<String>) -> Self {
        Self::Ready {
            handle: handle.into(),
        }
    }

    pub(crate) fn failed(stage: WorkerLaunchStage, error: impl Into<String>) -> Self {
        Self::Failed {
            error: HeadlessTermError::launch(stage, error),
        }
    }

    pub(crate) fn into_result(self) -> Result<(), HeadlessTermError> {
        match self {
            Self::Ready { .. } => Ok(()),
            Self::Failed { error } => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerLaunchReport;
    use crate::headlessterm::job::WorkerLaunchStage;

    #[test]
    fn launch_reports_are_tagged_json_values() {
        assert_eq!(
            serde_json::to_value(WorkerLaunchReport::ready("mira-kest")).unwrap(),
            serde_json::json!({"status": "ready", "handle": "mira-kest"})
        );
        assert_eq!(
            serde_json::to_value(WorkerLaunchReport::failed(
                WorkerLaunchStage::SpawnCommand,
                "docker not found",
            ))
            .unwrap(),
            serde_json::json!({
                "status": "failed",
                "error": {
                    "kind": "launch",
                    "stage": "spawn_command",
                    "error": "docker not found",
                },
            })
        );
    }
}
use crate::headlessterm::job::{HeadlessTermError, WorkerLaunchStage};
