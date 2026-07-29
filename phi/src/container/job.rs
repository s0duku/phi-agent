/*
 Job Container offers api to manage command job persistantly and backendly
*/

pub enum JobStatus {
    Running,
    Exited(i8),
    NoExist,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct TerminalSnapshot {
    revision: u64,
    text: String,
    #[serde(default)]
    stream: String,
    #[serde(default)]
    screen: String,
    #[serde(default)]
    truncated: bool,
    rows: u16,
    columns: u16,
    cursor_row: u16,
    cursor_column: u16,
}

pub struct JobInfo {
    status: JobStatus,
    terminal: TerminalSnapshot,
    waited: std::time::Duration,
}

pub struct JobHandle(pub String);

/// An access request for a running job.
#[derive(serde::Deserialize, serde::Serialize)]
pub enum JobAccess {
    /// Write input without waiting for or acquiring an output snapshot.
    Write { data: String },
    /// Write input and acquire an output snapshot.
    ///
    /// `wait` is the maximum duration to wait for the job to exit or for terminal
    /// output activity to settle. Output activity is used as a heuristic that
    /// meaningful new output is ready, so the request returns after the activity
    /// is followed by the configured quiet period. With no output activity, it
    /// waits for the full duration.
    Interact {
        data: String,
        wait: std::time::Duration,
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

#[async_trait::async_trait]
pub trait JobContainer {
    /// Start a command and acquire its initial output snapshot.
    ///
    /// `wait` bounds only the initial wait for output activity to settle or for
    /// the process to exit. Output activity acts as a heuristic that meaningful
    /// initial output is ready.
    /// `expiration` is the inactivity lifetime of a still-running container;
    /// once it elapses without another access, the container terminates the job
    /// and releases its resources.
    async fn job_exec(
        cmd: &str,
        wait: std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<JobHandle>, JobInfo), String>;
    async fn job_access(handle: JobHandle, access: JobAccess) -> Result<JobAccessResult, String>;

    async fn job_close(handle: JobHandle) -> Result<JobInfo, String>;
}

impl JobInfo {
    pub(crate) fn new(
        status: JobStatus,
        terminal: TerminalSnapshot,
        waited: std::time::Duration,
    ) -> Self {
        Self {
            status,
            terminal,
            waited,
        }
    }

    pub fn status(&self) -> &JobStatus {
        &self.status
    }

    pub fn terminal(&self) -> &TerminalSnapshot {
        &self.terminal
    }

    pub fn outputs(&self) -> &str {
        self.terminal.text()
    }

    /// Time spent waiting for process exit or terminal output to settle.
    /// Non-interactive access paths report zero.
    pub fn waited(&self) -> std::time::Duration {
        self.waited
    }

    pub fn into_parts(self) -> (JobStatus, TerminalSnapshot, std::time::Duration) {
        (self.status, self.terminal, self.waited)
    }
}

impl TerminalSnapshot {
    pub(crate) fn new(
        revision: u64,
        text: String,
        stream: String,
        screen: String,
        truncated: bool,
        rows: u16,
        columns: u16,
        cursor_row: u16,
        cursor_column: u16,
    ) -> Self {
        Self {
            revision,
            text,
            truncated,
            stream,
            screen,
            rows,
            columns,
            cursor_row,
            cursor_column,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn screen(&self) -> &str {
        &self.screen
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.columns)
    }

    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_row, self.cursor_column)
    }
}
