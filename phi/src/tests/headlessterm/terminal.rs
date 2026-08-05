use crate::headlessterm::worker::state::TerminalState;

#[test]
fn cursor_report_is_answered_and_not_exposed_as_command_output() {
    let mut terminal = TerminalState::new();
    let first = terminal.process(b"hello");
    assert!(first.activity.displayed());
    assert_eq!(terminal.rendered_output(), "hello");

    let query = terminal.process(b"\x1b[6n");
    assert!(!query.activity.displayed());
    assert_eq!(terminal.rendered_output(), "hello");
    assert_eq!(query.replies, [b"\x1b[1;6R".to_vec()]);
}

#[test]
fn fragmented_terminal_queries_remain_protocol_messages() {
    let mut terminal = TerminalState::new();
    assert!(!terminal.process(b"\x1b[").activity.displayed());
    assert_eq!(terminal.rendered_output(), "");

    let query = terminal.process(b"6n");
    assert!(!query.activity.displayed());
    assert_eq!(terminal.rendered_output(), "");
    assert_eq!(query.replies, [b"\x1b[1;1R".to_vec()]);
}

#[test]
fn queries_are_removed_without_disturbing_adjacent_output() {
    let mut terminal = TerminalState::new();
    let update = terminal.process(b"before\x1b[6nafter\r\n");

    assert!(update.activity.displayed());
    assert_eq!(terminal.rendered_output(), "beforeafter");
    assert_eq!(update.replies, [b"\x1b[1;7R".to_vec()]);
}

#[test]
fn utf8_split_across_pty_reads_is_not_corrupted() {
    let mut terminal = TerminalState::new();
    let bytes = "进度".as_bytes();

    assert!(!terminal.process(&bytes[..2]).activity.displayed());
    let middle = terminal.process(&bytes[2..5]);
    let tail = terminal.process(&bytes[5..]);

    assert!(middle.activity.displayed() || tail.activity.displayed());
    assert_eq!(terminal.rendered_output(), "进度");
}

#[test]
fn display_controls_are_preserved_while_the_screen_tracks_refreshes() {
    let mut terminal = TerminalState::new();
    let update = terminal.process(b"working 1\rworking 2\x08!\r\x1b[2Kdone");

    assert!(update.activity.displayed());
    assert!(update.replies.is_empty());
    assert_eq!(terminal.rendered_output(), "done");
}

#[test]
fn private_display_controls_keep_their_wire_order() {
    let mut terminal = TerminalState::new();
    let update = terminal.process(b"\x1b[?25lbusy\x1b[?25h");

    assert!(update.activity.displayed());
    assert_eq!(terminal.rendered_output(), "busy");
}

#[test]
fn empty_screen_control_sequences_do_not_create_visible_output() {
    let mut terminal = TerminalState::new();
    let update = terminal.process(b"\x1b[2J\x1b[H");
    let observation = terminal.observe(update.activity);

    assert!(update.activity.displayed());
    assert!(!observation.changed_since_checkpoint());
    assert_eq!(terminal.rendered_output(), "");
}

#[test]
fn device_and_size_queries_have_headlessterm_responses() {
    let mut terminal = TerminalState::new();
    let update = terminal.process(b"\x1b[5n\x1b[c\x1b[>c\x1b[18t");

    assert!(!update.activity.displayed());
    assert_eq!(terminal.rendered_output(), "");
    assert_eq!(
        update.replies,
        [
            b"\x1b[0n".to_vec(),
            b"\x1b[?1;2c".to_vec(),
            b"\x1b[>0;0;0c".to_vec(),
            b"\x1b[8;24;80t".to_vec(),
        ]
    );
}

#[test]
fn output_is_read_as_a_stream_delta_and_preserves_terminal_controls() {
    let mut terminal = TerminalState::new();
    terminal.process(b"first\n\x1b[2Ksecond\x1b[6n");
    let (output, truncated) = terminal.output();

    assert_eq!(output, "first\n\x1b[2Ksecond");
    assert!(!truncated);
    assert_eq!(terminal.output(), (output, false));
    terminal.acknowledge_output();
    assert_eq!(terminal.output(), (String::new(), false));
}

#[test]
fn observation_pins_delta_to_one_checkpoint_candidate() {
    let mut terminal = TerminalState::new();
    let first = terminal.process(b"first");
    let observation = terminal.observe(first.activity);
    terminal.process(b"\r\x1b[2Ksecond");

    let (output, truncated, delivery) = terminal.pending_response(&observation).into_parts();
    assert_eq!(output, "first");
    assert!(!truncated);
    terminal.acknowledge(delivery);

    assert_eq!(terminal.output().0, "\r\x1b[2Ksecond");
    assert_eq!(terminal.rendered_output(), "second");
}

#[test]
fn identical_screen_redraw_is_observable_without_a_visible_change() {
    let mut terminal = TerminalState::new();
    terminal.process(b"same");
    terminal.acknowledge_output();
    let update = terminal.process(b"\r\x1b[2Ksame");
    let observation = terminal.observe(update.activity);
    assert!(update.activity.displayed());
    assert!(!observation.changed_since_checkpoint());
    assert!(terminal.has_output());
}

#[test]
fn alternate_screen_activity_is_preserved_across_terminal_updates() {
    let mut terminal = TerminalState::new();
    let entered = terminal.process(b"\x1b[?1049hloading");
    let refreshed = terminal.process(b"\r\x1b[2Kready");

    assert!(entered.activity.alternate_screen_activity());
    assert!(refreshed.activity.alternate_screen_activity());
}

#[test]
fn bounded_output_reports_when_old_lines_were_discarded() {
    let mut terminal = TerminalState::new();
    for index in 0..1_001 {
        terminal.process(format!("line-{index}\n").as_bytes());
    }

    let (output, truncated) = terminal.output();
    assert!(truncated);
    assert!(!output.contains("line-0\n"));
    assert!(output.contains("line-1000\n"));
}

#[test]
fn initial_rendered_output_includes_scrollback_and_visible_screen() {
    let mut terminal = TerminalState::new();
    for index in 1..=40 {
        terminal.process(format!("line-{index}\r\n").as_bytes());
    }

    let output = terminal.rendered_output();
    assert!(output.starts_with("line-1\n"));
    assert!(output.contains("line-40"));
    assert_eq!(output.lines().count(), 40);

    terminal.acknowledge_output();
    for index in 41..=80 {
        terminal.process(format!("line-{index}\r\n").as_bytes());
    }
    let output = terminal.rendered_output();
    assert!(output.starts_with("line-41\n"));
    assert!(output.contains("line-80"));
    assert_eq!(output.lines().count(), 40);
}

#[test]
fn rendered_output_after_acknowledgement_only_contains_changed_rows() {
    let mut terminal = TerminalState::new();
    terminal.process(b"header\r\nworking");
    terminal.acknowledge_output();

    terminal.process(b"\r\x1b[2Kdone");
    assert_eq!(terminal.rendered_output(), "done");
}
