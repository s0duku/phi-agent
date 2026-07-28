use vte::{Params, Perform};

use crate::container::job::TerminalSnapshot;

const ROWS: u16 = 24;
const COLS: u16 = 80;
const SCROLLBACK_ROWS: usize = 1_000;

pub(crate) struct HeadlessTerminal {
    parser: vte::Parser,
    dispatcher: Dispatcher,
    screen: vt100::Parser,
    revision: u64,
}

pub(crate) struct TerminalUpdate {
    pub(crate) changed: bool,
    pub(crate) replies: Vec<Vec<u8>>,
}

impl HeadlessTerminal {
    pub(crate) fn new() -> Self {
        Self {
            parser: vte::Parser::new(),
            dispatcher: Dispatcher::default(),
            screen: vt100::Parser::new(ROWS, COLS, SCROLLBACK_ROWS),
            revision: 0,
        }
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) -> TerminalUpdate {
        let before = self.screen.screen().contents_formatted();
        self.parser.advance(&mut self.dispatcher, bytes);

        let mut replies = Vec::new();
        for event in self.dispatcher.events.drain(..) {
            match event {
                TerminalEvent::Display(bytes) => {
                    self.screen.process(&bytes);
                }
                TerminalEvent::Query(query) => {
                    replies.push(query.reply(self.screen.screen()));
                }
            }
        }
        let changed = before != self.screen.screen().contents_formatted();
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
        TerminalUpdate { changed, replies }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn snapshot(&self) -> TerminalSnapshot {
        let screen = self.screen.screen();
        let (cursor_row, cursor_column) = screen.cursor_position();
        TerminalSnapshot::new(
            self.revision,
            screen.contents(),
            ROWS,
            COLS,
            cursor_row,
            cursor_column,
        )
    }
}

enum TerminalEvent {
    Display(Vec<u8>),
    Query(TerminalQuery),
}

enum TerminalQuery {
    DeviceStatus,
    CursorPosition { private: bool },
    PrimaryDeviceAttributes,
    SecondaryDeviceAttributes,
    TextAreaSize,
    WindowSize,
}

impl TerminalQuery {
    fn reply(self, screen: &vt100::Screen) -> Vec<u8> {
        match self {
            Self::DeviceStatus => b"\x1b[0n".to_vec(),
            Self::CursorPosition { private } => {
                let (row, col) = screen.cursor_position();
                format!(
                    "\x1b[{}{};{}R",
                    if private { "?" } else { "" },
                    row + 1,
                    col + 1
                )
                .into_bytes()
            }
            Self::PrimaryDeviceAttributes => b"\x1b[?1;2c".to_vec(),
            Self::SecondaryDeviceAttributes => b"\x1b[>0;0;0c".to_vec(),
            Self::TextAreaSize => format!("\x1b[8;{ROWS};{COLS}t").into_bytes(),
            Self::WindowSize => b"\x1b[4;0;0t".to_vec(),
        }
    }
}

#[derive(Default)]
struct Dispatcher {
    events: Vec<TerminalEvent>,
    dcs: Option<Vec<u8>>,
}

impl Dispatcher {
    fn display(&mut self, bytes: impl AsRef<[u8]>) {
        let bytes = bytes.as_ref();
        if bytes.is_empty() {
            return;
        }
        if let Some(TerminalEvent::Display(output)) = self.events.last_mut() {
            output.extend_from_slice(bytes);
        } else {
            self.events.push(TerminalEvent::Display(bytes.to_vec()));
        }
    }

    fn query(&mut self, query: TerminalQuery) {
        self.events.push(TerminalEvent::Query(query));
    }
}

impl Perform for Dispatcher {
    fn print(&mut self, c: char) {
        let mut bytes = [0; 4];
        self.display(c.encode_utf8(&mut bytes).as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        if let Some(dcs) = &mut self.dcs {
            dcs.push(byte);
        } else {
            self.display([byte]);
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        if let Some(query) = terminal_query(params, intermediates, action) {
            self.query(query);
            return;
        }
        self.display(encode_csi(params, intermediates, action));
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        let mut sequence = Vec::with_capacity(intermediates.len() + 2);
        sequence.push(0x1b);
        sequence.extend_from_slice(intermediates);
        sequence.push(byte);
        self.display(sequence);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let mut sequence = b"\x1b]".to_vec();
        for (index, param) in params.iter().enumerate() {
            if index > 0 {
                sequence.push(b';');
            }
            sequence.extend_from_slice(param);
        }
        if bell_terminated {
            sequence.push(0x07);
        } else {
            sequence.extend_from_slice(b"\x1b\\");
        }
        self.display(sequence);
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.dcs = Some(encode_control_string(b'P', params, intermediates, action));
    }

    fn put(&mut self, byte: u8) {
        if let Some(dcs) = &mut self.dcs {
            dcs.push(byte);
        }
    }

    fn unhook(&mut self) {
        if let Some(mut sequence) = self.dcs.take() {
            sequence.extend_from_slice(b"\x1b\\");
            self.display(sequence);
        }
    }
}

fn terminal_query(params: &Params, intermediates: &[u8], action: char) -> Option<TerminalQuery> {
    let first = first_param(params);
    match (intermediates, first, action) {
        (b"", 5, 'n') => Some(TerminalQuery::DeviceStatus),
        (b"", 6, 'n') => Some(TerminalQuery::CursorPosition { private: false }),
        (b"?", 6, 'n') => Some(TerminalQuery::CursorPosition { private: true }),
        (b"", 0, 'c') => Some(TerminalQuery::PrimaryDeviceAttributes),
        (b">", 0, 'c') => Some(TerminalQuery::SecondaryDeviceAttributes),
        (b"", 18, 't') => Some(TerminalQuery::TextAreaSize),
        (b"", 14, 't') => Some(TerminalQuery::WindowSize),
        _ => None,
    }
}

fn first_param(params: &Params) -> u16 {
    params
        .iter()
        .next()
        .and_then(|param| param.first())
        .copied()
        .unwrap_or(0)
}

fn encode_csi(params: &Params, intermediates: &[u8], action: char) -> Vec<u8> {
    encode_sequence(b'[', params, intermediates, action)
}

fn encode_control_string(
    introducer: u8,
    params: &Params,
    intermediates: &[u8],
    action: char,
) -> Vec<u8> {
    encode_sequence(introducer, params, intermediates, action)
}

fn encode_sequence(introducer: u8, params: &Params, intermediates: &[u8], action: char) -> Vec<u8> {
    let mut sequence = vec![0x1b, introducer];
    let private_len = intermediates
        .iter()
        .take_while(|byte| matches!(byte, b'<' | b'=' | b'>' | b'?'))
        .count();
    sequence.extend_from_slice(&intermediates[..private_len]);
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            sequence.push(b';');
        }
        for (subindex, value) in param.iter().enumerate() {
            if subindex > 0 {
                sequence.push(b':');
            }
            sequence.extend_from_slice(value.to_string().as_bytes());
        }
    }
    sequence.extend_from_slice(&intermediates[private_len..]);
    let mut bytes = [0; 4];
    sequence.extend_from_slice(action.encode_utf8(&mut bytes).as_bytes());
    sequence
}
