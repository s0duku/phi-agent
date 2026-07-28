use std::sync::Arc;

use crate::executor::{PhiTool, tools::shell::shell_job_tools};

pub(crate) fn default_tools() -> Vec<Arc<dyn PhiTool>> {
    shell_job_tools()
}

#[cfg(test)]
mod tests {
    use super::default_tools;

    #[test]
    fn defaults_use_persistent_job_tools_only() {
        let names = default_tools()
            .into_iter()
            .map(|tool| tool.name().to_owned())
            .collect::<Vec<_>>();
        let shell = if cfg!(windows) {
            "powershell_job"
        } else {
            "bash_job"
        };

        assert_eq!(names, vec![shell, "job_interact", "job_close"]);
        assert!(
            !names
                .iter()
                .any(|name| name == "bash" || name == "powershell")
        );
    }
}
