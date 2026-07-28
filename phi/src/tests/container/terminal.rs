use crate::container::local::terminal::HeadlessTerminal;

#[test]
fn cursor_report_is_answered_and_not_exposed_as_command_output() {
    let mut terminal = HeadlessTerminal::new();
    let first = terminal.process(b"hello");
    assert!(first.output);
    assert_eq!(terminal.snapshot().revision(), 1);

    let query = terminal.process(b"\x1b[6n");
    assert!(!query.output);
    assert_eq!(terminal.snapshot().revision(), 1);
    assert_eq!(query.replies, [b"\x1b[1;6R".to_vec()]);
}

#[test]
fn fragmented_terminal_queries_remain_protocol_messages() {
    let mut terminal = HeadlessTerminal::new();
    assert!(!terminal.process(b"\x1b[").output);
    assert_eq!(terminal.snapshot().revision(), 0);

    let query = terminal.process(b"6n");
    assert!(!query.output);
    assert_eq!(terminal.snapshot().revision(), 0);
    assert_eq!(query.replies, [b"\x1b[1;1R".to_vec()]);
}

#[test]
fn queries_are_removed_without_disturbing_adjacent_output() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"before\x1b[6nafter\r\n");

    assert!(update.output);
    assert_eq!(terminal.snapshot().revision(), 1);
    assert_eq!(update.replies, [b"\x1b[1;7R".to_vec()]);
}

#[test]
fn utf8_split_across_pty_reads_is_not_corrupted() {
    let mut terminal = HeadlessTerminal::new();
    let bytes = "进度".as_bytes();

    assert!(!terminal.process(&bytes[..2]).output);
    let middle = terminal.process(&bytes[2..5]);
    let tail = terminal.process(&bytes[5..]);

    assert!(middle.output || tail.output);
    assert!(terminal.snapshot().revision() > 0);
    assert_eq!(terminal.snapshot().text(), "进度");
}

#[test]
fn display_controls_are_preserved_while_the_screen_tracks_refreshes() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"working 1\rworking 2\x08!\r\x1b[2Kdone");

    assert!(update.output);
    assert_eq!(terminal.snapshot().revision(), 1);
    assert!(update.replies.is_empty());
    assert_eq!(terminal.snapshot().text(), "done");
}

#[test]
fn private_display_controls_keep_their_wire_order() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"\x1b[?25lbusy\x1b[?25h");

    assert!(update.output);
    assert_eq!(terminal.snapshot().revision(), 1);
    assert_eq!(terminal.snapshot().text(), "busy");
}

#[test]
fn device_and_size_queries_have_headless_terminal_responses() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"\x1b[5n\x1b[c\x1b[>c\x1b[18t");

    assert!(!update.output);
    assert_eq!(terminal.snapshot().revision(), 0);
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
    let mut terminal = HeadlessTerminal::new();
    terminal.process(b"first\n\x1b[2Ksecond\x1b[6n");
    let (output, truncated) = terminal.output();

    assert_eq!(output, "first\n\x1b[2Ksecond");
    assert!(!truncated);
    assert_eq!(terminal.output(), (output, false));
    terminal.commit_output();
    assert_eq!(terminal.output(), (String::new(), false));
}

#[test]
fn identical_screen_redraw_still_counts_as_output_activity() {
    let mut terminal = HeadlessTerminal::new();
    terminal.process(b"same");
    terminal.commit_output();
    let revision = terminal.snapshot().revision();

    let update = terminal.process(b"\r\x1b[2Ksame");
    assert_eq!(terminal.snapshot().revision(), revision);
    assert!(update.output);
    assert!(terminal.has_output());
}

#[test]
fn bounded_output_reports_when_old_lines_were_discarded() {
    let mut terminal = HeadlessTerminal::new();
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
    let mut terminal = HeadlessTerminal::new();
    for index in 1..=40 {
        terminal.process(format!("line-{index}\r\n").as_bytes());
    }

    let output = terminal.rendered_output();
    assert!(output.starts_with("line-1\n"));
    assert!(output.contains("line-40"));
    assert_eq!(output.lines().count(), 40);

    terminal.commit_output();
    for index in 41..=80 {
        terminal.process(format!("line-{index}\r\n").as_bytes());
    }
    let output = terminal.rendered_output();
    assert!(output.starts_with("line-41\n"));
    assert!(output.contains("line-80"));
    assert_eq!(output.lines().count(), 40);
}

#[test]
fn rendered_output_after_commit_only_contains_changed_rows() {
    let mut terminal = HeadlessTerminal::new();
    terminal.process(b"header\r\nworking");
    terminal.commit_output();

    terminal.process(b"\r\x1b[2Kdone");
    assert_eq!(terminal.rendered_output(), "done");
}
