use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    agent::PhiAgentRuntime,
    executor::{PhiTool, PhiToolResult, ToolCallRequest, ToolCallResponse},
    headlessterm::{
        DEFAULT_TRY_WAIT, HeadlessTerminal, JobAccess, JobAccessResult, JobHandle, JobInfo,
        JobStatus, ReturnWhen, TerminalCommand,
    },
};

const EXEC_WAIT: Duration = DEFAULT_TRY_WAIT;
const JOB_EXPIRATION: Duration = Duration::from_secs(10 * 60);
const DEFAULT_INTERACT_WAIT_MS: u64 = DEFAULT_TRY_WAIT.as_millis() as u64;
const SUBMIT_KEY_DELAY: Duration = Duration::from_millis(20);

pub struct ShellJobExecTool;
pub struct ShellJobInteractTool;
pub struct ShellJobCloseTool;

#[derive(Deserialize)]
struct ExecArgs {
    cmd: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InteractArgs {
    handle: String,
    #[serde(default)]
    pub(crate) input: Option<String>,
    #[serde(default = "default_interact_wait_ms")]
    pub(crate) wait_ms: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InteractiveInput {
    Direct(String),
    Submit(String),
}

#[derive(Deserialize)]
struct CloseArgs {
    handle: String,
}

#[async_trait]
impl PhiTool for ShellJobExecTool {
    fn name(&self) -> &str {
        if cfg!(windows) {
            "powershell_job"
        } else {
            "bash_job"
        }
    }

    fn description(&self) -> &str {
        if cfg!(windows) {
            "Run a PowerShell command as a persistent job. The result status distinguishes exited output, settled linear output, a sampled interactive screen, and maximum-wait expiry; use the returned handle to continue a running job."
        } else {
            "Run a bash command as a persistent job. The result status distinguishes exited output, settled linear output, a sampled interactive screen, and maximum-wait expiry; use the returned handle to continue a running job."
        }
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "Shell command to execute"
                }
            },
            "required": ["cmd"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        runtime: &PhiAgentRuntime,
    ) -> PhiToolResult {
        let args = match parse_args::<ExecArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return Ok(failure(request, self.name(), error)),
        };
        if let Err(error) = prepare_local_jobs() {
            return Ok(failure(request, self.name(), error));
        }
        let terminal = HeadlessTerminal::new();
        let command = runtime
            .config()
            .command()
            .container()
            .map(|container| TerminalCommand::docker_exec(container, args.cmd.clone()))
            .unwrap_or_else(|| TerminalCommand::shell(args.cmd.clone()));
        let result = terminal
            .exec_job(
                command,
                ReturnWhen::output_settled(EXEC_WAIT),
                JOB_EXPIRATION,
            )
            .await;

        match result {
            Ok((handle, info)) => Ok(response(request, self.name(), info, handle)),
            Err(error) => Err(Box::new(error)),
        }
    }
}

#[async_trait]
impl PhiTool for ShellJobInteractTool {
    fn name(&self) -> &str {
        "job_interact"
    }

    fn description(&self) -> &str {
        "Interact with a running job and consume its newly available output. Status running_output_settled means linear output became quiet but the job may produce more later. Status running_screen_sampled is a bounded snapshot of an interactive screen that may still be updating. Status running_wait_elapsed means the maximum wait elapsed, so output may be a stage snapshot and should be read again. Omit input to only read output; provide an empty string for pressing Enter once; non-empty unterminated input is submitted with one carriage return."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "handle": {
                    "type": "string",
                    "description": "Handle of an already-running program returned by the shell job tool"
                },
                "input": {
                    "type": "string",
                    "description": "Optional input for the running program. Omit it to only read newly available output. Provide an empty string to press Enter once. Non-empty input without a trailing CR or LF is automatically followed by one CR, equivalent to pressing Enter once"
                },
                "wait_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "default": DEFAULT_INTERACT_WAIT_MS,
                    "description": "Maximum time to wait, in milliseconds, for exit or settled output. If this limit wins, status is running_wait_elapsed and output may be a stage snapshot of a still-changing terminal"
                }
            },
            "required": ["handle"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _runtime: &PhiAgentRuntime,
    ) -> PhiToolResult {
        let args = match parse_args::<InteractArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return Ok(failure(request, self.name(), error)),
        };
        if let Err(error) = prepare_local_jobs() {
            return Ok(failure(request, self.name(), error));
        }
        let handle_value = args.handle.clone();
        let handle = JobHandle(args.handle);
        let wait = Duration::from_millis(args.wait_ms);
        let result = match interactive_input(args.input) {
            InteractiveInput::Direct(data) => interact(handle, data, wait).await,
            InteractiveInput::Submit(data) => {
                let terminal = HeadlessTerminal::new();
                let written = terminal
                    .access_job(JobHandle(handle.0.clone()), JobAccess::Write { data })
                    .await;
                match written {
                    Ok(JobAccessResult::Written(_)) => {}
                    Ok(JobAccessResult::Interacted(_)) => {
                        return Ok(failure(
                            request,
                            self.name(),
                            "job access returned a terminal snapshot for write request".into(),
                        ));
                    }
                    Err(error) => return Err(Box::new(error)),
                }
                tokio::time::sleep(SUBMIT_KEY_DELAY).await;
                interact(handle, "\r".into(), wait).await
            }
        };

        match result {
            Ok(info) => Ok(response(
                request,
                self.name(),
                info,
                Some(JobHandle(handle_value)),
            )),
            Err(error) => Err(Box::new(error)),
        }
    }
}

async fn interact(
    handle: JobHandle,
    data: String,
    wait: Duration,
) -> Result<JobInfo, crate::headlessterm::HeadlessTermError> {
    let terminal = HeadlessTerminal::new();
    let result = terminal
        .access_job(
            handle,
            JobAccess::Interact {
                data,
                return_when: ReturnWhen::output_settled(wait),
            },
        )
        .await?;
    match result {
        JobAccessResult::Interacted(info) => Ok(info),
        JobAccessResult::Written(_) => Err(crate::headlessterm::HeadlessTermError::protocol(
            "job access returned a write acknowledgment for interact request",
        )),
    }
}

#[async_trait]
impl PhiTool for ShellJobCloseTool {
    fn name(&self) -> &str {
        "job_close"
    }

    fn description(&self) -> &str {
        "Read current output, stop a local job, and release its resources. A successfully stopped process returns status closed with its final exit code."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "handle": {
                    "type": "string",
                    "description": "Job handle returned by the shell job tool"
                }
            },
            "required": ["handle"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _runtime: &PhiAgentRuntime,
    ) -> PhiToolResult {
        let args = match parse_args::<CloseArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return Ok(failure(request, self.name(), error)),
        };
        if let Err(error) = prepare_local_jobs() {
            return Ok(failure(request, self.name(), error));
        }
        let result = HeadlessTerminal::new()
            .close_job(JobHandle(args.handle))
            .await;

        match result {
            Ok(info) => Ok(response(request, self.name(), info, None)),
            Err(error) => Err(Box::new(error)),
        }
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    request: &ToolCallRequest,
    name: &str,
) -> Result<T, String> {
    serde_json::from_value(request.arguments.clone())
        .map_err(|error| format!("invalid {name} tool arguments: {error}"))
}

fn response(
    request: &ToolCallRequest,
    name: &str,
    info: JobInfo,
    handle: Option<JobHandle>,
) -> ToolCallResponse {
    let (status, output, truncated, waited) = info.into_parts();
    let (status_name, exit_code, running) = match status {
        JobStatus::RunningOutputSettled => ("running_output_settled", None, true),
        JobStatus::RunningScreenSampled => ("running_screen_sampled", None, true),
        JobStatus::RunningWaitElapsed => ("running_wait_elapsed", None, true),
        JobStatus::Exited(code) => ("exited", Some(code), false),
        JobStatus::Closed(code) => ("closed", Some(code), false),
        JobStatus::NoExist => ("not_found", None, false),
    };
    let handle = if running {
        handle.map(|handle| handle.0)
    } else {
        None
    };
    let value = serde_json::json!({
        "status": status_name,
        "exit_code": exit_code,
        "output": output,
        "truncated": truncated,
        "handle": handle,
        "waited_ms": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
    });

    ToolCallResponse::new(request, name, value)
}

fn failure(request: &ToolCallRequest, name: &str, error: String) -> ToolCallResponse {
    ToolCallResponse::new(request, name, serde_json::json!({"error": error}))
}

pub(crate) fn interactive_input(input: Option<String>) -> InteractiveInput {
    let Some(mut input) = input else {
        return InteractiveInput::Direct(String::new());
    };
    if input.is_empty() {
        return InteractiveInput::Direct("\r".to_owned());
    }
    if cfg!(windows) {
        input = input.replace("\r\n", "\r").replace('\n', "\r");
    }
    if input.ends_with('\r') || input.ends_with('\n') {
        InteractiveInput::Direct(input)
    } else {
        InteractiveInput::Submit(input)
    }
}

fn prepare_local_jobs() -> Result<(), String> {
    static JOBS: OnceLock<()> = OnceLock::new();
    if JOBS.get().is_some() {
        return Ok(());
    }
    install_local_jobs()?;
    let _ = JOBS.set(());
    Ok(())
}

fn install_local_jobs() -> Result<(), String> {
    Ok(())
}

const fn default_interact_wait_ms() -> u64 {
    DEFAULT_INTERACT_WAIT_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_response_reports_actual_wait_duration() {
        let request = ToolCallRequest {
            id: "call-1".into(),
            call_id: None,
            name: "job_interact".into(),
            arguments: serde_json::json!({}),
        };
        let info = JobInfo::new(
            JobStatus::RunningWaitElapsed,
            String::new(),
            false,
            Duration::from_millis(123),
        );

        let response = response(
            &request,
            "job_interact",
            info,
            Some(JobHandle("mira-kest".into())),
        );

        assert_eq!(response.output.as_value()["waited_ms"], 123);
        assert_eq!(response.output.as_value()["status"], "running_wait_elapsed");
        assert_eq!(response.output.as_value()["truncated"], false);
        assert!(response.output.as_value().get("output_truncated").is_none());
        assert!(response.output.as_value().get("screen").is_none());
    }

    #[test]
    fn job_status_does_not_define_phi_tool_success() {
        let request = ToolCallRequest {
            id: "call-1".into(),
            call_id: None,
            name: "bash_job".into(),
            arguments: serde_json::json!({}),
        };

        for (status, expected_status, expected_exit_code) in [
            (JobStatus::Exited(17), "exited", serde_json::json!(17)),
            (JobStatus::Closed(9), "closed", serde_json::json!(9)),
            (JobStatus::NoExist, "not_found", serde_json::Value::Null),
        ] {
            let response = response(
                &request,
                "bash_job",
                JobInfo::new(status, String::new(), false, Duration::ZERO),
                None,
            );

            assert_eq!(response.output.as_value()["status"], expected_status);
            assert_eq!(response.output.as_value()["exit_code"], expected_exit_code);
        }
    }

    #[test]
    fn running_job_status_exposes_the_interaction_boundary() {
        let request = ToolCallRequest {
            id: "call-1".into(),
            call_id: None,
            name: "job_interact".into(),
            arguments: serde_json::json!({}),
        };

        for (status, expected) in [
            (JobStatus::RunningOutputSettled, "running_output_settled"),
            (JobStatus::RunningScreenSampled, "running_screen_sampled"),
            (JobStatus::RunningWaitElapsed, "running_wait_elapsed"),
        ] {
            let response = response(
                &request,
                "job_interact",
                JobInfo::new(status, String::new(), false, Duration::ZERO),
                Some(JobHandle("mira-kest".into())),
            );
            assert_eq!(response.output.as_value()["status"], expected);
            assert_eq!(response.output.as_value()["handle"], "mira-kest");
        }
    }
}
