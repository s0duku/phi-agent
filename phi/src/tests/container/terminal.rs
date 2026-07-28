use crate::container::local::terminal::HeadlessTerminal;

#[test]
fn cursor_report_is_answered_and_not_exposed_as_command_output() {
    let mut terminal = HeadlessTerminal::new();
    let first = terminal.process(b"hello");
    assert!(first.changed);

    let query = terminal.process(b"\x1b[6n");
    assert!(!query.changed);
    assert_eq!(query.replies, [b"\x1b[1;6R".to_vec()]);
}

#[test]
fn fragmented_terminal_queries_remain_protocol_messages() {
    let mut terminal = HeadlessTerminal::new();
    assert!(!terminal.process(b"\x1b[").changed);

    let query = terminal.process(b"6n");
    assert!(!query.changed);
    assert_eq!(query.replies, [b"\x1b[1;1R".to_vec()]);
}

#[test]
fn queries_are_removed_without_disturbing_adjacent_output() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"before\x1b[6nafter\r\n");

    assert!(update.changed);
    assert_eq!(update.replies, [b"\x1b[1;7R".to_vec()]);
}

#[test]
fn utf8_split_across_pty_reads_is_not_corrupted() {
    let mut terminal = HeadlessTerminal::new();
    let bytes = "进度".as_bytes();

    assert!(!terminal.process(&bytes[..2]).changed);
    let middle = terminal.process(&bytes[2..5]);
    let tail = terminal.process(&bytes[5..]);

    assert!(middle.changed || tail.changed);
    assert_eq!(terminal.snapshot().text(), "进度");
}

#[test]
fn display_controls_are_preserved_while_the_screen_tracks_refreshes() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"working 1\rworking 2\x08!\r\x1b[2Kdone");

    assert!(update.changed);
    assert!(update.replies.is_empty());
    assert_eq!(terminal.snapshot().text(), "done");
}

#[test]
fn private_display_controls_keep_their_wire_order() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"\x1b[?25lbusy\x1b[?25h");

    assert!(update.changed);
    assert_eq!(terminal.snapshot().text(), "busy");
}

#[test]
fn device_and_size_queries_have_headless_terminal_responses() {
    let mut terminal = HeadlessTerminal::new();
    let update = terminal.process(b"\x1b[5n\x1b[c\x1b[>c\x1b[18t");

    assert!(!update.changed);
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
