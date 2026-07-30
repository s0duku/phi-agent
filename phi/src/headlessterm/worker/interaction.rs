//! Wait semantics for one terminal interaction.
//!
//! `InteractionState::begin` creates a transaction that consumes checkpoint-relative
//! terminal observations until the requested wait or output-settle boundary is reached.

use std::time::{Duration, Instant};

use crate::headlessterm::job::ReturnWhen;

use super::state::TerminalObservation;

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);
const OUTPUT_SETTLE_PERIOD: Duration = POLL_INTERVAL.saturating_mul(20);

#[derive(Default)]
pub(crate) struct InteractionState {
    settle_deadline: Option<Instant>,
}

pub(crate) struct TerminalInteraction {
    wait_deadline: Option<Instant>,
    settle_deadline: Option<Instant>,
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

    pub(crate) fn remaining(&self) -> Option<Duration> {
        self.remaining_at(Instant::now())
    }

    pub(crate) fn remaining_at(&self, now: Instant) -> Option<Duration> {
        let wait = remaining(self.wait_deadline, now)?;
        let settle = remaining(self.settle_deadline, now)?;
        Some(wait.min(settle))
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

fn update_settle_deadline(deadline: &mut Option<Instant>, class: ObservationClass, at: Instant) {
    match class {
        ObservationClass::LinearChange => {
            *deadline = at.checked_add(OUTPUT_SETTLE_PERIOD);
        }
        ObservationClass::InteractiveScreenChange => {
            if deadline.is_none() {
                *deadline = at.checked_add(OUTPUT_SETTLE_PERIOD);
            }
        }
        ObservationClass::None | ObservationClass::RepeatedDisplay => {}
    }
}

fn remaining(deadline: Option<Instant>, now: Instant) -> Option<Duration> {
    match deadline {
        Some(deadline) => deadline.checked_duration_since(now),
        None => Some(Duration::MAX),
    }
}
