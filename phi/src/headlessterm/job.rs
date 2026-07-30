use std::time::Duration;

pub const DEFAULT_TRY_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HeadlessTermError {
    Launch {
        stage: WorkerLaunchStage,
        error: String,
    },
    Transport {
        operation: String,
        error: String,
    },
    Protocol {
        error: String,
    },
    Operation {
        error: String,
    },
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLaunchStage {
    DecodeCommand,
    BindRpc,
    SpawnCommand,
    SpawnWorker,
    AwaitWorker,
}

impl HeadlessTermError {
    pub(crate) fn launch(stage: WorkerLaunchStage, error: impl Into<String>) -> Self {
        Self::Launch {
            stage,
            error: error.into(),
        }
    }

    pub(crate) fn transport(operation: impl Into<String>, error: impl Into<String>) -> Self {
        Self::Transport {
            operation: operation.into(),
            error: error.into(),
        }
    }

    pub(crate) fn protocol(error: impl Into<String>) -> Self {
        Self::Protocol {
            error: error.into(),
        }
    }

    pub(crate) fn operation(error: impl Into<String>) -> Self {
        Self::Operation {
            error: error.into(),
        }
    }
}

impl std::fmt::Display for HeadlessTermError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch { stage, error } => {
                write!(formatter, "worker launch failed at {stage}: {error}")
            }
            Self::Transport { operation, error } => {
                write!(
                    formatter,
                    "headlessterm transport failed during {operation}: {error}"
                )
            }
            Self::Protocol { error } => write!(formatter, "headlessterm protocol error: {error}"),
            Self::Operation { error } => {
                write!(formatter, "headlessterm operation failed: {error}")
            }
        }
    }
}

impl std::error::Error for HeadlessTermError {}

impl std::fmt::Display for WorkerLaunchStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::DecodeCommand => "decode_command",
            Self::BindRpc => "bind_rpc",
            Self::SpawnCommand => "spawn_command",
            Self::SpawnWorker => "spawn_worker",
            Self::AwaitWorker => "await_worker",
        };
        formatter.write_str(name)
    }
}

/// A command interpreted by the headlessterm worker.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub enum TerminalCommand {
    Shell {
        command: String,
    },
    DockerExec {
        container: String,
        command: String,
        #[serde(default = "default_container_shell")]
        shell: String,
    },
}

fn default_container_shell() -> String {
    "/bin/sh".to_owned()
}

/// The boundary that completes one terminal interaction.
#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
pub enum ReturnWhen {
    OutputSettled { try_wait: Duration },
}

pub enum JobStatus {
    Running,
    Exited(i8),
    NoExist,
}

pub struct JobInfo {
    status: JobStatus,
    output: String,
    truncated: bool,
    waited: std::time::Duration,
}

pub struct JobHandle(pub String);

/// An access request for a running job.
#[derive(serde::Deserialize, serde::Serialize)]
pub enum JobAccess {
    /// Write input without waiting for or acquiring an output delta.
    Write { data: String },
    /// Write input and acquire the output delta since the previous interaction.
    ///
    Interact {
        data: String,
        return_when: ReturnWhen,
    },
}

pub enum JobAccessResult {
    Written(JobStatus),
    Interacted(JobInfo),
}

impl JobHandle {
    const LETTER_COUNT: usize = 8;
    pub(crate) const ENCODED_LENGTH: usize = Self::LETTER_COUNT + 1;
    const SEPARATOR_INDEX: usize = Self::LETTER_COUNT / 2;
    const UNBIASED_BYTE_LIMIT: u8 = 26 * (u8::MAX / 26);

    pub(crate) fn random() -> Result<Self, String> {
        let mut handle = String::with_capacity(Self::ENCODED_LENGTH);
        let mut random = [0_u8; 16];
        while handle.len() < Self::ENCODED_LENGTH {
            getrandom::fill(&mut random).map_err(|error| error.to_string())?;
            for byte in random {
                if byte >= Self::UNBIASED_BYTE_LIMIT {
                    continue;
                }
                if handle.len() == Self::SEPARATOR_INDEX {
                    handle.push('-');
                }
                handle.push(char::from(b'a' + byte % 26));
                if handle.len() == Self::ENCODED_LENGTH {
                    break;
                }
            }
        }
        Ok(Self(handle))
    }

    pub(crate) fn is_valid(value: &str) -> bool {
        value.len() == Self::ENCODED_LENGTH
            && value.bytes().enumerate().all(|(index, byte)| {
                if index == Self::SEPARATOR_INDEX {
                    byte == b'-'
                } else {
                    byte.is_ascii_lowercase()
                }
            })
    }
}

impl TerminalCommand {
    pub fn shell(command: impl Into<String>) -> Self {
        Self::Shell {
            command: command.into(),
        }
    }

    pub fn docker_exec(container: impl Into<String>, command: impl Into<String>) -> Self {
        Self::DockerExec {
            container: container.into(),
            command: command.into(),
            shell: default_container_shell(),
        }
    }
}

impl From<String> for TerminalCommand {
    fn from(command: String) -> Self {
        Self::shell(command)
    }
}

impl From<&str> for TerminalCommand {
    fn from(command: &str) -> Self {
        Self::shell(command)
    }
}

impl From<&String> for TerminalCommand {
    fn from(command: &String) -> Self {
        Self::shell(command)
    }
}

impl ReturnWhen {
    pub const fn output_settled(try_wait: Duration) -> Self {
        Self::OutputSettled { try_wait }
    }
}

impl From<Duration> for ReturnWhen {
    fn from(try_wait: Duration) -> Self {
        Self::output_settled(try_wait)
    }
}

impl JobInfo {
    pub(crate) fn new(
        status: JobStatus,
        output: String,
        truncated: bool,
        waited: std::time::Duration,
    ) -> Self {
        Self {
            status,
            output,
            truncated,
            waited,
        }
    }

    pub fn status(&self) -> &JobStatus {
        &self.status
    }

    pub fn outputs(&self) -> &str {
        &self.output
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Time spent waiting for process exit or terminal output to settle.
    /// Non-interactive access paths report zero.
    pub fn waited(&self) -> std::time::Duration {
        self.waited
    }

    pub fn into_parts(self) -> (JobStatus, String, bool, std::time::Duration) {
        (self.status, self.output, self.truncated, self.waited)
    }
}
