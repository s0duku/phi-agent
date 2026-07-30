use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use portable_pty::{Child, PtySize, native_pty_system};

use crate::headlessterm::job::TerminalCommand;

use super::state::{
    PendingTerminalResponse, TerminalActivity, TerminalDelivery, TerminalObservation, TerminalState,
};
use super::{command, platform};

pub(crate) struct PtySession {
    _master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output_rx: Receiver<Vec<u8>>,
    terminal: TerminalState,
    eof: bool,
    exit_status: Option<i8>,
    #[cfg(unix)]
    process_group_leader: Option<libc::pid_t>,
}

impl PtySession {
    pub(crate) fn spawn(command: TerminalCommand) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())?;
        platform::disable_pty_echo(&*pair.master).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        let process_group_leader = pair.master.process_group_leader();
        let builder = command::build(command)?;
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let (output_tx, output_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if output_tx.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            _master: pair.master,
            child,
            writer,
            output_rx,
            terminal: TerminalState::new(),
            eof: false,
            exit_status: None,
            #[cfg(unix)]
            process_group_leader,
        })
    }

    pub(crate) fn capture(&mut self) -> Result<TerminalObservation, String> {
        let mut activity = TerminalActivity::default();
        loop {
            match self.output_rx.try_recv() {
                Ok(chunk) => {
                    let update = self.terminal.process(&chunk);
                    for reply in update.replies {
                        // A ConPTY may close its input side before its child
                        // handle reports the final status. The reply is best
                        // effort and must not turn a successful command into
                        // an RPC failure.
                        let _ = self.writer.write_all(&reply);
                    }
                    activity.merge(update.activity);
                }
                Err(TryRecvError::Empty) => return Ok(self.terminal.observe(activity)),
                Err(TryRecvError::Disconnected) => {
                    self.eof = true;
                    return Ok(self.terminal.observe(activity));
                }
            }
        }
    }

    pub(crate) fn write_all(&mut self, data: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(data)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn refresh_status(&mut self) -> Result<Option<i8>, String> {
        if self.exit_status.is_some() {
            return Ok(self.exit_status);
        }
        #[cfg(unix)]
        {
            if let Some(code) = platform::poll_process_status(self.child.process_id())
                .map_err(|error| error.to_string())?
            {
                self.exit_status = Some(code);
                return Ok(Some(code));
            }
        }
        #[cfg(windows)]
        {
            if let Some(code) = platform::poll_process_status(&mut *self.child)
                .map_err(|error| error.to_string())?
            {
                self.exit_status = Some(code);
                return Ok(Some(code));
            }
        }

        let status = self
            .child
            .try_wait()
            .map_err(|error| error.to_string())
            .map(|status| status.map(|status| status.exit_code() as u8 as i8))?;
        if let Some(code) = status {
            self.exit_status = Some(code);
        }
        Ok(status)
    }

    pub(crate) fn terminate(&mut self, force: bool) -> Result<(), String> {
        #[cfg(unix)]
        {
            let _ = platform::kill_process(self.process_group_leader, force);
            match self.child.kill() {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
        #[cfg(windows)]
        {
            platform::kill_process(&mut *self.child, force).map_err(|error| error.to_string())
        }
    }

    pub(crate) fn reached_eof(&self) -> bool {
        self.eof
    }

    pub(crate) fn pending_response(
        &self,
        observation: &TerminalObservation,
    ) -> PendingTerminalResponse {
        self.terminal.pending_response(observation)
    }

    pub(crate) fn acknowledge(&mut self, delivery: TerminalDelivery) {
        self.terminal.acknowledge(delivery);
    }

    pub(crate) fn current_observation(&self) -> TerminalObservation {
        self.terminal.observe(TerminalActivity::default())
    }
}
