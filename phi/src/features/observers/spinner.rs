use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use owo_colors::{OwoColorize, Stream, Style};
use tokio::{task::JoinHandle, time::sleep};

use crate::{
    error::PhiAgentRuntimeResult,
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
};

const LABEL_ROTATE_SECS: u64 = 3;
const THINKING_LABELS: &[&str] = &[
    "thinking",
    "connecting dots",
    "chasing a hunch",
    "untangling context",
    "interviewing the evidence",
    "bribing the gremlins",
    "shaking the codebase awake",
    "following suspicious breadcrumbs",
    "asking the rubber duck",
    "massaging the stack trace",
    "recombobulating thoughts",
    "coaxing the model",
    "spelunking for answers",
    "making the clues line up",
    "negotiating with entropy",
    "performing light wizardry",
    "reading the tea leaves",
    "poking at assumptions",
    "turning the puzzle around",
    "squinting at the details",
    "listening for loose threads",
    "dusting for fingerprints",
    "cross-examining the context",
    "staring down the weird part",
    "walking the theory back",
    "checking the obvious twice",
    "asking better questions",
    "replaying the scene",
    "looking under the floorboards",
    "lining up the alibis",
    "stress-testing the hunch",
    "doing suspiciously legal magic",
    "making friends with the logs",
    "coaching the clues",
    "tugging on loose ends",
    "untying the knot gently",
    "following the scent trail",
    "sorting signal from noise",
    "turning over one more rock",
    "trying the least silly idea",
    "trying the most silly idea",
    "keeping the gremlins busy",
    "asking the code nicely",
    "negotiating with the stack",
    "reading between the warnings",
    "looking for the missing stair",
    "shuffling theories around",
    "shining a flashlight on it",
    "counting the moving parts",
    "checking where the story bends",
    "holding the pieces together",
    "comparing notes with reality",
    "gently interrogating the bug",
    "waiting for the pattern to blink",
    "making the weirdness confess",
    "translating chaos into clues",
    "stitching the timeline together",
    "peeling back another layer",
    "trying not to overfit the mystery",
    "asking whether that can really be true",
    "measuring twice, guessing once",
    "looking for the load-bearing detail",
    "offending the problem with reason",
    "bringing order to the nonsense",
    "double-checking the trapdoor",
    "tapping on the walls for hollow spots",
    "following the smoke",
    "taking the scenic route through logic",
    "looking for where the truth squeaks",
];
const BRAILLE_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const QUARTER_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];
const CLOCK_FRAMES: &[&str] = &["◴", "◷", "◶", "◵"];
const ARC_FRAMES: &[&str] = &["◜", "◠", "◝", "◞", "◡", "◟"];
const PIXEL_CORNERS_FRAMES: &[&str] = &["▖", "▘", "▝", "▗"];
const PIXEL_BAR_FRAMES: &[&str] = &["▁", "▃", "▅", "▇", "▅", "▃"];
const PIXEL_SHADE_FRAMES: &[&str] = &["░", "▒", "▓", "█", "▓", "▒"];
const PIXEL_BOX_FRAMES: &[&str] = &["┤", "┘", "┴", "└", "├", "┌", "┬", "┐"];
const ASCII_FRAMES: &[&str] = &["-", "\\", "|", "/"];

const UTF8_STYLES: &[SpinnerStyle] = &[
    SpinnerStyle {
        frames: BRAILLE_FRAMES,
        tick_ms: 90,
    },
    SpinnerStyle {
        frames: QUARTER_FRAMES,
        tick_ms: 140,
    },
    SpinnerStyle {
        frames: CLOCK_FRAMES,
        tick_ms: 160,
    },
    SpinnerStyle {
        frames: ARC_FRAMES,
        tick_ms: 130,
    },
    SpinnerStyle {
        frames: PIXEL_CORNERS_FRAMES,
        tick_ms: 140,
    },
    SpinnerStyle {
        frames: PIXEL_BAR_FRAMES,
        tick_ms: 120,
    },
    SpinnerStyle {
        frames: PIXEL_SHADE_FRAMES,
        tick_ms: 140,
    },
    SpinnerStyle {
        frames: PIXEL_BOX_FRAMES,
        tick_ms: 110,
    },
];

const ASCII_STYLES: &[SpinnerStyle] = &[SpinnerStyle {
    frames: ASCII_FRAMES,
    tick_ms: 100,
}];

pub struct SpinnerModule {
    active: Option<ActiveSpinner>,
}

struct ActiveSpinner {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

struct SpinnerStyle {
    frames: &'static [&'static str],
    tick_ms: u64,
}

impl SpinnerModule {
    pub fn new() -> Self {
        Self { active: None }
    }

    fn start(&mut self, label: String) {
        self.stop();

        if !io::stderr().is_terminal() {
            return;
        }

        let _ = writeln!(io::stderr().lock());
        let _ = io::stderr().lock().flush();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_signal = Arc::clone(&stop);
        let label_index_seed = random_seed();
        let style = random_spinner_style();
        let handle = tokio::spawn(async move {
            let mut frame_index = 0usize;
            let started_at = Instant::now();

            loop {
                if stop_signal.load(Ordering::Relaxed) {
                    clear_line();
                    return;
                }

                render_frame(
                    style.frames[frame_index % style.frames.len()],
                    display_label(&label, label_index_seed, started_at.elapsed()),
                    started_at.elapsed(),
                );
                frame_index = frame_index.wrapping_add(1);
                sleep(Duration::from_millis(style.tick_ms)).await;
            }
        });

        self.active = Some(ActiveSpinner { stop, handle });
    }

    fn stop(&mut self) {
        self.stop_with_newline(false);
    }

    fn stop_with_newline(&mut self, newline: bool) {
        let Some(active) = self.active.take() else {
            return;
        };

        active.stop.store(true, Ordering::Relaxed);
        active.handle.abort();
        clear_line();
        if newline {
            let _ = writeln!(io::stderr().lock());
            let _ = io::stderr().lock().flush();
        }
    }
}

impl Drop for SpinnerModule {
    fn drop(&mut self) {
        self.stop_with_newline(true);
    }
}

impl PhiModule for SpinnerModule {
    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        match event {
            PhiAgentStepEvent::BeforeCompactRequest { .. } => {
                self.start("compacting history".to_string());
            }
            PhiAgentStepEvent::AfterCompactResponse { .. } => {
                self.stop();
            }
            PhiAgentStepEvent::BeforeModelRequest { .. } => {
                self.start("thinking".to_string());
            }
            PhiAgentStepEvent::AfterModelResponseParsed { .. } => {}
            PhiAgentStepEvent::AfterModelResponse { .. }
            | PhiAgentStepEvent::AfterToolCall { .. } => {
                self.stop();
            }
            PhiAgentStepEvent::BeforeToolCall { request, .. } => {
                self.start(format!("running {}", request.name));
            }
            PhiAgentStepEvent::BeforeCreateNextStep { .. }
            | PhiAgentStepEvent::BeforeReplaceBaseStep { .. } => {}
        }

        Ok(())
    }

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        match event {
            PhiAgentCommitEvent::ModelResponseCommitted { .. }
            | PhiAgentCommitEvent::MessageCommitted { .. }
            | PhiAgentCommitEvent::WarningEmitted { .. }
            | PhiAgentCommitEvent::StepFailed { .. } => self.stop(),
        }
    }
}

fn render_frame(frame: &str, label: &str, elapsed: Duration) {
    let spinner = frame
        .if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().green().bold())
        })
        .to_string();
    let label = label
        .if_supports_color(Stream::Stderr, |text| text.style(Style::new().green()))
        .to_string();
    let elapsed = format_elapsed(elapsed)
        .if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().bright_black())
        })
        .to_string();

    let _ = write!(io::stderr().lock(), "\r\x1b[2K{spinner} {label} {elapsed}");
    let _ = io::stderr().lock().flush();
}

fn clear_line() {
    let _ = write!(io::stderr().lock(), "\r\x1b[2K\r");
    let _ = io::stderr().lock().flush();
}

fn format_elapsed(elapsed: Duration) -> String {
    let total_seconds = elapsed.as_secs();
    let hours = total_seconds / 3600;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        let remaining_minutes = (total_seconds % 3600) / 60;
        return format!("{hours}h {remaining_minutes:02}m {seconds:02}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds:02}s");
    }
    format!("{seconds}s")
}

fn display_label(base_label: &str, seed: usize, elapsed: Duration) -> &str {
    if base_label != "thinking" {
        return base_label;
    }

    let slot = (elapsed.as_secs() / LABEL_ROTATE_SECS) as usize;
    THINKING_LABELS[(seed + slot) % THINKING_LABELS.len()]
}

fn random_seed() -> usize {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(0);
    seed % THINKING_LABELS.len()
}

fn random_spinner_style() -> SpinnerStyle {
    let styles = if env_looks_utf8() {
        UTF8_STYLES
    } else {
        ASCII_STYLES
    };

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(0);
    let index = seed % styles.len();

    SpinnerStyle {
        frames: styles[index].frames,
        tick_ms: styles[index].tick_ms,
    }
}

fn env_looks_utf8() -> bool {
    std::env::var("LC_ALL")
        .ok()
        .or_else(|| std::env::var("LC_CTYPE").ok())
        .or_else(|| std::env::var("LANG").ok())
        .map(|value| {
            let value = value.to_ascii_uppercase();
            value.contains("UTF-8") || value.contains("UTF8")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{ASCII_STYLES, UTF8_STYLES};

    #[test]
    fn spinner_frames_are_stable_single_cells_without_emoji() {
        for style in UTF8_STYLES.iter().chain(ASCII_STYLES) {
            assert!(!style.frames.is_empty());
            for frame in style.frames {
                assert_eq!(frame.chars().count(), 1, "frame: {frame}");
                let scalar = frame.chars().next().unwrap() as u32;
                assert!(
                    !(0x1f000..=0x1faff).contains(&scalar),
                    "emoji spinner frame: {frame}"
                );
            }
        }
    }

    #[test]
    fn non_utf8_spinner_styles_are_ascii_only() {
        assert!(
            ASCII_STYLES
                .iter()
                .flat_map(|style| style.frames)
                .all(|frame| frame.is_ascii())
        );
    }
}
