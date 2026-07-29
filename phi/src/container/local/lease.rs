use std::time::{Duration, Instant};

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
