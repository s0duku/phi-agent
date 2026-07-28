use std::time::{Duration, Instant};

use crate::container::local::wait::{ActivityExpiration, WaitPolicy};

#[test]
fn activity_expiration_applies_to_running_jobs() {
    let start = Instant::now();
    let lease = ActivityExpiration::new_at(Duration::from_secs(10), Duration::ZERO, start);

    assert!(!lease.elapsed_at(start + Duration::from_secs(9)));
    assert!(lease.elapsed_at(start + Duration::from_secs(10)));
}

#[test]
fn interaction_restarts_activity_expiration() {
    let start = Instant::now();
    let mut lease = ActivityExpiration::new_at(Duration::from_secs(10), Duration::ZERO, start);
    lease.observe_interaction_at(start + Duration::from_secs(9));

    assert!(!lease.elapsed_at(start + Duration::from_secs(18)));
    assert!(lease.elapsed_at(start + Duration::from_secs(19)));
}

#[test]
fn command_exit_restarts_activity_expiration() {
    let start = Instant::now();
    let mut lease = ActivityExpiration::new_at(Duration::from_secs(10), Duration::ZERO, start);
    lease.observe_exit_at(start + Duration::from_secs(9));

    assert!(!lease.elapsed_at(start + Duration::from_secs(18)));
    assert!(lease.elapsed_at(start + Duration::from_secs(19)));
}

#[test]
fn startup_grace_only_applies_before_first_interaction() {
    let start = Instant::now();
    let mut lease =
        ActivityExpiration::new_at(Duration::from_millis(100), Duration::from_secs(5), start);
    assert!(!lease.elapsed_at(start + Duration::from_secs(1)));

    lease.observe_interaction_at(start + Duration::from_secs(1));
    assert!(!lease.elapsed_at(start + Duration::from_millis(1099)));
    assert!(lease.elapsed_at(start + Duration::from_millis(1100)));
}

#[test]
fn output_adds_a_shorter_quiet_deadline() {
    let mut wait = WaitPolicy::new(Duration::from_secs(5), None);
    assert!(wait.remaining().unwrap() > Duration::from_secs(4));

    wait.observe_output(true);
    let remaining = wait.remaining().unwrap();
    assert!(remaining > Duration::from_millis(1400));
    assert!(remaining <= Duration::from_millis(1500));
}

#[test]
fn existing_output_starts_the_quiet_deadline() {
    let wait = WaitPolicy::new(Duration::from_secs(5), Some(Instant::now()));
    let remaining = wait.remaining().unwrap();
    assert!(remaining > Duration::from_millis(1400));
    assert!(remaining <= Duration::from_millis(1500));
}

#[test]
fn output_already_quiet_long_enough_returns_immediately() {
    let wait = WaitPolicy::new(
        Duration::from_secs(5),
        Some(Instant::now() - Duration::from_secs(2)),
    );
    assert!(wait.remaining().is_none());
}

#[test]
fn an_unrepresentable_timeout_does_not_expire_immediately() {
    let wait = WaitPolicy::new(Duration::MAX, None);
    assert_eq!(wait.remaining(), Some(Duration::MAX));
}
