use std::time::{Duration, Instant};

use crate::headlessterm::ReturnWhen;
use crate::headlessterm::worker::interaction::InteractionState;
use crate::headlessterm::worker::lease::ActivityExpiration;
use crate::headlessterm::worker::state::TerminalObservation;

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
fn linear_output_adds_a_shorter_settle_deadline() {
    let now = Instant::now();
    let state = InteractionState::default();
    let mut interaction = state.begin_at(ReturnWhen::output_settled(Duration::from_secs(5)), now);
    interaction.observe(&TerminalObservation::from_facts(true, true, false), now);

    assert_eq!(
        interaction.remaining_at(now + Duration::from_millis(100)),
        Some(Duration::from_millis(2900))
    );
}

#[test]
fn pending_output_carries_its_settle_deadline_into_a_new_wait() {
    let now = Instant::now();
    let mut state = InteractionState::default();
    state.observe(&TerminalObservation::from_facts(true, true, false), now);
    let interaction = state.begin_at(Duration::from_secs(5), now);

    assert_eq!(interaction.remaining_at(now), Some(Duration::from_secs(3)));
}

#[test]
fn output_already_settled_returns_immediately() {
    let now = Instant::now();
    let mut state = InteractionState::default();
    state.observe(
        &TerminalObservation::from_facts(true, true, false),
        now - Duration::from_secs(4),
    );
    let interaction = state.begin_at(Duration::from_secs(5), now);

    assert_eq!(interaction.remaining_at(now), None);
}

#[test]
fn an_unrepresentable_wait_does_not_expire_immediately() {
    let interaction = InteractionState::default().begin(Duration::MAX);
    assert_eq!(interaction.remaining(), Some(Duration::MAX));
}

#[test]
fn repeated_display_does_not_shorten_the_requested_wait() {
    let now = Instant::now();
    let state = InteractionState::default();
    let mut interaction = state.begin_at(Duration::from_secs(5), now);
    interaction.observe(&TerminalObservation::from_facts(true, false, false), now);

    assert_eq!(interaction.remaining_at(now), Some(Duration::from_secs(5)));
}

#[test]
fn interactive_screen_activity_has_a_bounded_sample_window() {
    let now = Instant::now();
    let state = InteractionState::default();
    let mut interaction = state.begin_at(Duration::from_secs(5), now);
    let observation = TerminalObservation::from_facts(true, true, true);
    interaction.observe(&observation, now);
    interaction.observe(&observation, now + Duration::from_secs(1));

    assert_eq!(
        interaction.remaining_at(now + Duration::from_millis(1400)),
        Some(Duration::from_millis(1600))
    );
}

#[test]
fn acknowledging_delivery_clears_pending_interaction_activity() {
    let now = Instant::now();
    let mut state = InteractionState::default();
    state.observe(&TerminalObservation::from_facts(true, true, false), now);
    state.acknowledge();
    let interaction = state.begin_at(Duration::from_secs(5), now);

    assert_eq!(interaction.remaining_at(now), Some(Duration::from_secs(5)));
}
