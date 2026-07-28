use std::time::{Duration, Instant};

pub(crate) const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const UNCHANGED_PROBES: u32 = 15;
const OUTPUT_QUIET_PERIOD: Duration = PROBE_INTERVAL.saturating_mul(UNCHANGED_PROBES);

pub(crate) struct ActivityExpiration {
    expiration: Duration,
    startup_grace: Duration,
    last_activity: Instant,
    interacted: bool,
}

impl ActivityExpiration {
    pub(crate) fn new(expiration: Duration, startup_grace: Duration) -> Self {
        Self::new_at(expiration, startup_grace, Instant::now())
    }

    pub(crate) fn observe_interaction(&mut self) {
        self.observe_interaction_at(Instant::now());
    }

    pub(crate) fn observe_exit(&mut self) {
        self.observe_exit_at(Instant::now());
    }

    pub(crate) fn elapsed(&self) -> bool {
        self.elapsed_at(Instant::now())
    }

    fn active_expiration(&self) -> Duration {
        if self.interacted {
            self.expiration
        } else {
            self.expiration.max(self.startup_grace)
        }
    }

    pub(crate) fn new_at(expiration: Duration, startup_grace: Duration, now: Instant) -> Self {
        Self {
            expiration,
            startup_grace,
            last_activity: now,
            interacted: false,
        }
    }

    pub(crate) fn observe_interaction_at(&mut self, now: Instant) {
        self.last_activity = now;
        self.interacted = true;
    }

    pub(crate) fn observe_exit_at(&mut self, now: Instant) {
        self.last_activity = now;
    }

    pub(crate) fn elapsed_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_activity) >= self.active_expiration()
    }
}

pub(crate) struct WaitPolicy {
    deadline: Option<Instant>,
    quiet_deadline: Option<Instant>,
}

impl WaitPolicy {
    pub(crate) fn new(timeout: Duration, last_output_at: Option<Instant>) -> Self {
        let now = Instant::now();
        Self {
            deadline: now.checked_add(timeout),
            quiet_deadline: last_output_at.and_then(|at| at.checked_add(OUTPUT_QUIET_PERIOD)),
        }
    }

    pub(crate) fn observe_output(&mut self, changed: bool) {
        if changed {
            self.quiet_deadline = Some(Instant::now() + OUTPUT_QUIET_PERIOD);
        }
    }

    pub(crate) fn remaining(&self) -> Option<Duration> {
        let timeout = remaining(self.deadline)?;
        let quiet = remaining(self.quiet_deadline)?;
        Some(timeout.min(quiet))
    }
}

fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    match deadline {
        Some(deadline) => deadline.checked_duration_since(Instant::now()),
        None => Some(Duration::MAX),
    }
}
