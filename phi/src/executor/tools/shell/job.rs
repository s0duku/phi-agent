use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    agent::PhiAgentRuntime,
    container::{JobContainer, JobHandle, JobInfo, JobStatus, LocalShellJobContainer},
    executor::{PhiTool, ToolCallRequest, ToolCallResponse, ToolExecutionLimits},
};

const EXEC_WAIT: Duration = Duration::from_secs(60);
const JOB_EXPIRATION: Duration = Duration::from_secs(10 * 60);
const DEFAULT_INTERACT_WAIT_MS: u64 = 60_000;
const SUBMIT_KEY_DELAY: Duration = Duration::from_millis(20);

pub struct ShellJobExecTool;
pub struct ShellJobInteractTool;
pub struct ShellJobCloseTool;

#[derive(Deserialize)]
struct ExecArgs {
    cmd: String,
}

#[derive(Deserialize)]
pub(crate) struct InteractArgs {
    handle: String,
    #[serde(default, alias = "data")]
    pub(crate) input: Option<String>,
    #[serde(default = "default_interact_wait_ms")]
    pub(crate) timeout: u64,
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
            "Run a PowerShell command job."
        } else {
            "Run a bash command job."
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
        _limits: ToolExecutionLimits,
        _runtime: &PhiAgentRuntime,
    ) -> ToolCallResponse {
        let args = match parse_args::<ExecArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return failure(request, self.name(), error),
        };
        if let Err(error) = prepare_local_jobs() {
            return failure(request, self.name(), error);
        }
        let result = <LocalShellJobContainer as JobContainer>::job_exec(
            &args.cmd,
            EXEC_WAIT,
            JOB_EXPIRATION,
        )
        .await;

        match result {
            Ok((handle, info)) => response(request, self.name(), info, handle, false),
            Err(error) => failure(request, self.name(), error),
        }
    }
}

#[async_trait]
impl PhiTool for ShellJobInteractTool {
    fn name(&self) -> &str {
        "job_interact"
    }

    fn description(&self) -> &str {
        "Interact with a program that is already running under a local job handle and read its newly available output. Omit input to only read output. Provide an empty string to press Enter once. Non-empty input is submitted by appending one carriage return (CR), equivalent to pressing Enter once, when it has no trailing newline."
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
                "timeout": {
                    "type": "integer",
                    "minimum": 0,
                    "default": DEFAULT_INTERACT_WAIT_MS,
                    "description": "Maximum wait in milliseconds"
                }
            },
            "required": ["handle"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _limits: ToolExecutionLimits,
        _runtime: &PhiAgentRuntime,
    ) -> ToolCallResponse {
        let args = match parse_args::<InteractArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return failure(request, self.name(), error),
        };
        if let Err(error) = prepare_local_jobs() {
            return failure(request, self.name(), error);
        }
        let handle_value = args.handle.clone();
        let handle = JobHandle(args.handle);
        let timeout = Duration::from_millis(args.timeout);
        let result = match interactive_input(args.input) {
            InteractiveInput::Direct(data) => {
                <LocalShellJobContainer as JobContainer>::job_write(handle, &data, timeout).await
            }
            InteractiveInput::Submit(data) => {
                if let Err(error) = <LocalShellJobContainer as JobContainer>::job_send(
                    JobHandle(handle.0.clone()),
                    &data,
                )
                .await
                {
                    return failure(request, self.name(), error);
                }
                tokio::time::sleep(SUBMIT_KEY_DELAY).await;
                <LocalShellJobContainer as JobContainer>::job_write(handle, "\r", timeout).await
            }
        };

        match result {
            Ok(info) => response(
                request,
                self.name(),
                info,
                Some(JobHandle(handle_value)),
                false,
            ),
            Err(error) => failure(request, self.name(), error),
        }
    }
}

#[async_trait]
impl PhiTool for ShellJobCloseTool {
    fn name(&self) -> &str {
        "job_close"
    }

    fn description(&self) -> &str {
        "Read current output, stop a local job, and release its resources."
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
        _limits: ToolExecutionLimits,
        _runtime: &PhiAgentRuntime,
    ) -> ToolCallResponse {
        let args = match parse_args::<CloseArgs>(request, self.name()) {
            Ok(args) => args,
            Err(error) => return failure(request, self.name(), error),
        };
        if let Err(error) = prepare_local_jobs() {
            return failure(request, self.name(), error);
        }
        let result =
            <LocalShellJobContainer as JobContainer>::job_close(JobHandle(args.handle)).await;

        match result {
            Ok(info) => response(request, self.name(), info, None, true),
            Err(error) => failure(request, self.name(), error),
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
    closing: bool,
) -> ToolCallResponse {
    let (status, terminal) = info.into_parts();
    let output = terminal.text().to_owned();
    let output_truncated = terminal.truncated();
    let screen = terminal.screen().to_owned();
    let (status_name, exit_code, running, exists) = match status {
        JobStatus::Running => ("running", None, true, true),
        JobStatus::Exited(code) => ("exited", Some(code), false, true),
        JobStatus::NoExist => ("not_found", None, false, false),
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
        "output_truncated": output_truncated,
        "screen": screen,
        "handle": handle,
    });

    if !exists {
        return ToolCallResponse::failure(request, name, "job does not exist", value);
    }
    if let Some(code) = exit_code.filter(|code| !closing && *code != 0) {
        return ToolCallResponse::failure(
            request,
            name,
            format!("shell exited with status {code}"),
            value,
        );
    }
    ToolCallResponse::success(request, name, value)
}

fn failure(request: &ToolCallRequest, name: &str, error: String) -> ToolCallResponse {
    ToolCallResponse::failure(request, name, error, serde_json::Value::Null)
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
