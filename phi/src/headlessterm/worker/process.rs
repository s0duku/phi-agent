use std::time::{Duration, Instant};

use super::interaction::{self, InteractionBoundary, InteractionState};
use super::protocol::{ProcessStatus, Status};
use super::pty::PtySession;
use crate::headlessterm::job::{ReturnWhen, TerminalCommand};

use super::state::{PendingTerminalResponse, TerminalDelivery, TerminalObservation};

const POLL_INTERVAL: Duration = interaction::POLL_INTERVAL;
const CLOSE_GRACE: Duration = Duration::from_millis(250);
const FORCE_CLOSE_GRACE: Duration = Duration::from_secs(5);
const EXIT_OUTPUT_GRACE: Duration = Duration::from_millis(250);

pub(super) struct RunningJob {
    pty: PtySession,
    observation: TerminalObservation,
    interactions: InteractionState,
    exited_at: Option<Instant>,
}

pub(super) struct CompletedInteraction {
    status: Status,
    waited: Duration,
    pending: PendingTerminalResponse,
}

impl RunningJob {
    pub(super) fn spawn(command: TerminalCommand) -> Result<Self, String> {
        let mut pty = PtySession::spawn(command)?;
        let observation = pty.capture()?;
        let observed_at = Instant::now();
        let mut interactions = InteractionState::default();
        interactions.observe(&observation, observed_at);
        Ok(Self {
            pty,
            observation,
            interactions,
            exited_at: None,
        })
    }

    pub(super) fn observe_terminal(&mut self) -> Result<(), String> {
        self.observe_terminal_activity().map(|_| ())
    }

    pub(super) fn refresh_status(&mut self) -> Result<Option<i8>, String> {
        let status = self.pty.refresh_status()?;
        if status.is_some() && self.exited_at.is_none() {
            self.exited_at = Some(Instant::now());
        }
        Ok(status)
    }

    pub(super) fn has_exited(&self) -> bool {
        self.exited_at.is_some()
    }

    pub(super) fn expire(&mut self) -> Result<(), String> {
        if !self.pty.reached_eof() {
            self.pty.terminate(true)?;
        }
        Ok(())
    }

    pub(super) fn interact(
        &mut self,
        input: &[u8],
        return_when: ReturnWhen,
    ) -> Result<CompletedInteraction, String> {
        let started_at = Instant::now();
        if !input.is_empty() {
            self.pty.write_all(input)?;
        }
        let mut interaction = self.interactions.begin(return_when);
        loop {
            let observed_at = self.observe_terminal_activity()?;
            interaction.observe(&self.observation, observed_at);
            if let Some(code) = self.refresh_status()? {
                self.observe_after_exit()?;
                return self.complete_interaction(Status::Exited(code), started_at.elapsed());
            }
            match interaction.boundary() {
                InteractionBoundary::Pending(remaining) => {
                    std::thread::sleep(remaining.min(POLL_INTERVAL));
                }
                InteractionBoundary::OutputSettled => {
                    return self
                        .complete_interaction(Status::RunningOutputSettled, started_at.elapsed());
                }
                InteractionBoundary::ScreenSampled => {
                    return self
                        .complete_interaction(Status::RunningScreenSampled, started_at.elapsed());
                }
                InteractionBoundary::WaitElapsed => {
                    return self
                        .complete_interaction(Status::RunningWaitElapsed, started_at.elapsed());
                }
            }
        }
    }

    pub(super) fn write(&mut self, input: &[u8]) -> Result<ProcessStatus, String> {
        if !input.is_empty() {
            self.pty.write_all(input)?;
        }
        self.refresh_status()?
            .map(ProcessStatus::Exited)
            .map_or(Ok(ProcessStatus::Running), Ok)
    }

    pub(super) fn close(&mut self) -> Result<Status, String> {
        self.observe_terminal()?;
        if let Some(code) = self.refresh_status()? {
            self.observe_after_exit()?;
            return Ok(Status::Closed(code));
        }

        self.pty.terminate(false)?;
        if let Some(code) = self.wait_for_exit(CLOSE_GRACE)? {
            return Ok(Status::Closed(code));
        }

        self.pty.terminate(true)?;
        self.wait_for_exit(FORCE_CLOSE_GRACE)?
            .map(Status::Closed)
            .ok_or_else(|| "job did not stop".to_owned())
    }

    pub(super) fn reached_eof(&self) -> bool {
        self.pty.reached_eof()
    }

    pub(super) fn pending_response(&self) -> PendingTerminalResponse {
        self.pty.pending_response(&self.observation)
    }

    pub(super) fn acknowledge(&mut self, delivery: TerminalDelivery) {
        self.pty.acknowledge(delivery);
        self.observation = self.pty.current_observation();
        self.interactions.acknowledge();
    }

    fn complete_interaction(
        &mut self,
        status: Status,
        waited: Duration,
    ) -> Result<CompletedInteraction, String> {
        self.observe_terminal_activity()?;
        Ok(CompletedInteraction {
            status,
            waited,
            pending: self.pending_response(),
        })
    }

    fn observe_terminal_activity(&mut self) -> Result<Instant, String> {
        let terminal_observation = self.pty.capture()?;
        let observed_at = Instant::now();
        self.interactions
            .observe(&terminal_observation, observed_at);
        self.observation = terminal_observation;
        Ok(observed_at)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<i8>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.observe_terminal()?;
            if let Some(code) = self.refresh_status()? {
                self.observe_after_exit()?;
                return Ok(Some(code));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(
                POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    fn observe_after_exit(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + EXIT_OUTPUT_GRACE;
        while !self.pty.reached_eof() && Instant::now() < deadline {
            self.observe_terminal()?;
            if !self.pty.reached_eof() {
                std::thread::sleep(
                    POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        self.observe_terminal()
    }
}

impl CompletedInteraction {
    pub(super) fn into_parts(self) -> (Status, Duration, PendingTerminalResponse) {
        (self.status, self.waited, self.pending)
    }
}
