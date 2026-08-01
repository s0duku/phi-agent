//! Wait semantics for one terminal interaction.
//!
//! `InteractionState::begin` creates a transaction that consumes checkpoint-relative
//! terminal observations until the requested wait or output-settle boundary is reached.

use std::time::{Duration, Instant};

use crate::headlessterm::job::ReturnWhen;

use super::state::TerminalObservation;

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTPUT_SETTLE_PERIOD: Duration = POLL_INTERVAL.saturating_mul(30);

#[derive(Default)]
pub(crate) struct InteractionState {
    settle_deadline: Option<SettleDeadline>,
}

pub(crate) struct TerminalInteraction {
    wait_deadline: Option<Instant>,
    settle_deadline: Option<SettleDeadline>,
}

pub(crate) enum InteractionBoundary {
    Pending(Duration),
    OutputSettled,
    ScreenSampled,
    WaitElapsed,
}

#[derive(Clone, Copy)]
struct SettleDeadline {
    at: Instant,
    kind: SettleKind,
}

#[derive(Clone, Copy)]
enum SettleKind {
    OutputSettled,
    ScreenSampled,
}

#[derive(Clone, Copy)]
enum ObservationClass {
    None,
    RepeatedDisplay,
    LinearChange,
    InteractiveScreenChange,
}

impl InteractionState {
    pub(crate) fn observe(&mut self, observation: &TerminalObservation, at: Instant) {
        update_settle_deadline(&mut self.settle_deadline, classify(observation), at);
    }

    pub(crate) fn begin<W: Into<ReturnWhen>>(&self, return_when: W) -> TerminalInteraction {
        self.begin_at(return_when, Instant::now())
    }

    pub(crate) fn acknowledge(&mut self) {
        self.settle_deadline = None;
    }

    pub(crate) fn begin_at<W: Into<ReturnWhen>>(
        &self,
        return_when: W,
        now: Instant,
    ) -> TerminalInteraction {
        let ReturnWhen::OutputSettled { try_wait } = return_when.into();
        TerminalInteraction {
            wait_deadline: now.checked_add(try_wait),
            settle_deadline: self.settle_deadline,
        }
    }
}

impl TerminalInteraction {
    pub(crate) fn observe(&mut self, observation: &TerminalObservation, at: Instant) {
        update_settle_deadline(&mut self.settle_deadline, classify(observation), at);
    }

    #[cfg(test)]
    pub(crate) fn remaining(&self) -> Option<Duration> {
        self.remaining_at(Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn remaining_at(&self, now: Instant) -> Option<Duration> {
        match self.boundary_at(now) {
            InteractionBoundary::Pending(remaining) => Some(remaining),
            InteractionBoundary::OutputSettled
            | InteractionBoundary::ScreenSampled
            | InteractionBoundary::WaitElapsed => None,
        }
    }

    pub(crate) fn boundary(&self) -> InteractionBoundary {
        self.boundary_at(Instant::now())
    }

    pub(crate) fn boundary_at(&self, now: Instant) -> InteractionBoundary {
        match (self.wait_deadline, self.settle_deadline) {
            (Some(wait), Some(settle)) if settle.at <= wait => settle_boundary(settle, now),
            (Some(wait), _) => deadline_boundary(wait, now, InteractionBoundary::WaitElapsed),
            (None, Some(settle)) => settle_boundary(settle, now),
            (None, None) => InteractionBoundary::Pending(Duration::MAX),
        }
    }
}

fn classify(observation: &TerminalObservation) -> ObservationClass {
    let activity = observation.activity();
    if !activity.displayed() {
        return ObservationClass::None;
    }
    if !observation.changed_since_checkpoint() {
        return ObservationClass::RepeatedDisplay;
    }
    if activity.alternate_screen_activity() {
        ObservationClass::InteractiveScreenChange
    } else {
        ObservationClass::LinearChange
    }
}

fn update_settle_deadline(
    deadline: &mut Option<SettleDeadline>,
    class: ObservationClass,
    at: Instant,
) {
    match class {
        ObservationClass::LinearChange => {
            *deadline = at
                .checked_add(OUTPUT_SETTLE_PERIOD)
                .map(|at| SettleDeadline {
                    at,
                    kind: SettleKind::OutputSettled,
                });
        }
        ObservationClass::InteractiveScreenChange => {
            if deadline.is_none() {
                *deadline = at
                    .checked_add(OUTPUT_SETTLE_PERIOD)
                    .map(|at| SettleDeadline {
                        at,
                        kind: SettleKind::ScreenSampled,
                    });
            }
        }
        ObservationClass::None | ObservationClass::RepeatedDisplay => {}
    }
}

fn settle_boundary(deadline: SettleDeadline, now: Instant) -> InteractionBoundary {
    let elapsed = match deadline.kind {
        SettleKind::OutputSettled => InteractionBoundary::OutputSettled,
        SettleKind::ScreenSampled => InteractionBoundary::ScreenSampled,
    };
    deadline_boundary(deadline.at, now, elapsed)
}

fn deadline_boundary(
    deadline: Instant,
    now: Instant,
    elapsed: InteractionBoundary,
) -> InteractionBoundary {
    match deadline.checked_duration_since(now) {
        Some(remaining) if !remaining.is_zero() => InteractionBoundary::Pending(remaining),
        _ => elapsed,
    }
}
