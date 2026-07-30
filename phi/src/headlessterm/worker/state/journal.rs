use std::collections::VecDeque;

use vte::{Params, Perform};

use super::TerminalObservation;

pub(super) struct OutputJournal {
    bytes: VecDeque<u8>,
    line_count: usize,
    truncated: bool,
    start_offset: u64,
    end_offset: u64,
    checkpoint: TerminalCheckpoint,
}

pub(crate) struct OutputDelivery {
    checkpoint: TerminalCheckpoint,
}

struct TerminalCheckpoint {
    stream_offset: u64,
    rendered_rows: Vec<String>,
}

pub(super) struct OutputSelector;

pub(super) struct PendingOutput {
    pub(super) text: String,
    pub(super) truncated: bool,
    pub(super) delivery: OutputDelivery,
}

impl OutputJournal {
    pub(super) fn new() -> Self {
        Self {
            bytes: VecDeque::new(),
            line_count: 0,
            truncated: false,
            start_offset: 0,
            end_offset: 0,
            checkpoint: TerminalCheckpoint {
                stream_offset: 0,
                rendered_rows: Vec::new(),
            },
        }
    }

    pub(super) fn append(&mut self, bytes: &[u8], max_lines: usize, max_bytes: usize) {
        self.bytes.extend(bytes.iter().copied());
        self.line_count += bytes.iter().filter(|&&byte| byte == b'\n').count();
        self.end_offset = self
            .end_offset
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        while self.line_count > max_lines || self.bytes.len() > max_bytes {
            let Some(byte) = self.bytes.pop_front() else {
                break;
            };
            self.start_offset = self.start_offset.saturating_add(1);
            if byte == b'\n' {
                self.line_count = self.line_count.saturating_sub(1);
            }
            self.truncated = true;
        }
    }

    #[cfg(test)]
    pub(super) fn has_pending(&self) -> bool {
        !self.bytes.is_empty() || self.truncated
    }

    pub(super) fn changed_since_checkpoint(&self, rendered_rows: &[String]) -> bool {
        self.checkpoint.rendered_rows != rendered_rows
    }

    pub(super) fn end_offset(&self) -> u64 {
        self.end_offset
    }

    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }

    pub(super) fn stream(&self, observation: &TerminalObservation) -> String {
        let observed = observation
            .stream_end_offset()
            .saturating_sub(self.start_offset);
        let observed = usize::try_from(observed)
            .unwrap_or(usize::MAX)
            .min(self.bytes.len());
        let bytes: Vec<_> = self.bytes.iter().take(observed).copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(super) fn acknowledge(&mut self, delivery: OutputDelivery) {
        let delivered = delivery
            .checkpoint
            .stream_offset
            .saturating_sub(self.start_offset);
        let delivered = usize::try_from(delivered)
            .unwrap_or(usize::MAX)
            .min(self.bytes.len());
        self.bytes.drain(..delivered);
        self.start_offset = self
            .start_offset
            .saturating_add(u64::try_from(delivered).unwrap_or(u64::MAX));
        self.line_count = self.bytes.iter().filter(|&&byte| byte == b'\n').count();
        self.truncated = false;
        self.checkpoint = delivery.checkpoint;
    }
}

impl OutputSelector {
    pub(super) fn select(
        &self,
        journal: &OutputJournal,
        observation: &TerminalObservation,
    ) -> PendingOutput {
        let stream = journal.stream(observation);
        let text = Self::render(&stream, &journal.checkpoint, observation);
        PendingOutput {
            text,
            truncated: observation.stream_truncated(),
            delivery: OutputDelivery {
                checkpoint: TerminalCheckpoint {
                    stream_offset: observation.stream_end_offset(),
                    rendered_rows: observation.rendered_rows().to_vec(),
                },
            },
        }
    }

    fn render(
        stream: &str,
        checkpoint: &TerminalCheckpoint,
        observation: &TerminalObservation,
    ) -> String {
        linear_output(stream).unwrap_or_else(|| {
            rendered_difference(&checkpoint.rendered_rows, observation.rendered_rows())
        })
    }
}

fn rendered_difference(before: &[String], after: &[String]) -> String {
    let columns = after.len() + 1;
    let mut lengths = vec![0_u16; (before.len() + 1) * columns];
    for before_index in (0..before.len()).rev() {
        for after_index in (0..after.len()).rev() {
            let index = before_index * columns + after_index;
            lengths[index] = if before[before_index] == after[after_index] {
                lengths[(before_index + 1) * columns + after_index + 1].saturating_add(1)
            } else {
                lengths[(before_index + 1) * columns + after_index]
                    .max(lengths[before_index * columns + after_index + 1])
            };
        }
    }

    let mut before_index = 0;
    let mut after_index = 0;
    let mut changed = Vec::new();
    while before_index < before.len() && after_index < after.len() {
        if before[before_index] == after[after_index] {
            before_index += 1;
            after_index += 1;
        } else if lengths[(before_index + 1) * columns + after_index]
            >= lengths[before_index * columns + after_index + 1]
        {
            before_index += 1;
        } else {
            changed.push(after[after_index].as_str());
            after_index += 1;
        }
    }
    changed.extend(after[after_index..].iter().map(String::as_str));

    let start = changed
        .iter()
        .position(|line| !line.is_empty())
        .unwrap_or(changed.len());
    let end = changed
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(start, |index| index + 1);
    changed[start..end].join("\n")
}

fn linear_output(stream: &str) -> Option<String> {
    let mut parser = vte::Parser::new();
    let mut output = LinearOutput::default();
    parser.advance(&mut output, stream.as_bytes());
    output.finish();
    (!output.screen_oriented).then(|| output.text.trim_end_matches(['\r', '\n']).to_owned())
}

#[derive(Default)]
struct LinearOutput {
    text: String,
    screen_oriented: bool,
    pending_carriage_return: bool,
}

impl LinearOutput {
    fn flush_carriage_return(&mut self) {
        if std::mem::take(&mut self.pending_carriage_return) {
            self.screen_oriented = true;
        }
    }

    fn finish(&mut self) {
        self.flush_carriage_return();
    }
}

impl Perform for LinearOutput {
    fn print(&mut self, character: char) {
        self.flush_carriage_return();
        self.text.push(character);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => self.pending_carriage_return = true,
            b'\n' if self.pending_carriage_return => {
                self.pending_carriage_return = false;
                self.text.push('\n');
            }
            b'\n' | b'\t' => {
                self.flush_carriage_return();
                self.text.push(char::from(byte));
            }
            0x08 | 0x0b | 0x0c => {
                self.flush_carriage_return();
                self.screen_oriented = true;
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.flush_carriage_return();
        let alternate_screen = matches!(action, 'h' | 'l')
            && params
                .iter()
                .flatten()
                .any(|value| matches!(*value, 47 | 1047 | 1049));
        self.screen_oriented |= alternate_screen
            || matches!(
                action,
                '@' | 'A'
                    | 'B'
                    | 'C'
                    | 'D'
                    | 'E'
                    | 'F'
                    | 'G'
                    | 'H'
                    | 'J'
                    | 'K'
                    | 'L'
                    | 'M'
                    | 'P'
                    | 'S'
                    | 'T'
                    | 'X'
                    | 'd'
                    | 'e'
                    | 'f'
                    | 'r'
                    | 's'
                    | 'u'
            );
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
        self.flush_carriage_return();
        self.screen_oriented = true;
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        self.flush_carriage_return();
        self.screen_oriented = true;
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.flush_carriage_return();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputJournal, OutputSelector, TerminalObservation, linear_output, rendered_difference,
    };

    #[test]
    fn linear_crlf_and_styles_render_as_an_increment() {
        assert_eq!(
            linear_output("\x1b[?9001hfirst\r\nsecond\x1b[31m red\x1b[0m\r\n"),
            Some("first\nsecond red".to_owned())
        );
    }

    #[test]
    fn cursor_updates_require_terminal_state_rendering() {
        assert_eq!(linear_output("working\r\x1b[2Kdone"), None);
    }

    #[test]
    fn linear_increment_is_not_limited_by_terminal_height() {
        let stream = (1..=40)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        let output = linear_output(&stream).unwrap();
        assert_eq!(output.lines().count(), 40);
        assert!(output.starts_with("line-1\n"));
        assert!(output.ends_with("line-40"));
    }

    #[test]
    fn delivery_only_acknowledges_the_observed_prefix() {
        let mut output = OutputJournal::new();
        output.append(b"first\n", 1_000, 1024);
        let first =
            TerminalObservation::from_rendered_rows(vec!["first".to_owned()], output.end_offset);
        output.append(b"second\n", 1_000, 1024);
        let pending = OutputSelector.select(&output, &first);
        assert_eq!(output.stream(&first), "first\n");

        output.acknowledge(pending.delivery);
        let second = TerminalObservation::from_rendered_rows(
            vec!["first".to_owned(), "second".to_owned()],
            output.end_offset,
        );
        let pending = OutputSelector.select(&output, &second);
        assert_eq!(output.stream(&second), "second\n");
        assert_eq!(pending.text, "second");
    }

    #[test]
    fn rendered_difference_ignores_unchanged_rows_that_moved() {
        let before = [
            "old header",
            "model",
            "directory",
            "tip",
            "warning",
            "prompt",
            "status",
        ]
        .map(str::to_owned);
        let after = [
            "model",
            "directory",
            "tip",
            "warning",
            "你好",
            "assistant response",
            "prompt",
            "status",
        ]
        .map(str::to_owned);

        assert_eq!(
            rendered_difference(&before, &after),
            "你好\nassistant response"
        );
    }
}
