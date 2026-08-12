use std::collections::VecDeque;

use crate::message::{PhiHistory, PhiMessage, PhiReasoningContent};

use super::{LoopDetection, LoopDetector, similarity::NgramFingerprint};

#[derive(Clone, Debug)]
pub struct ReasoningSimilarityConfig {
    pub ngram_size: usize,
    pub similarity_threshold: f64,
    pub min_chars: usize,
}

pub struct ReasoningSimilarityDetector {
    config: ReasoningSimilarityConfig,
    committed: VecDeque<NgramFingerprint>,
    rejected: Vec<NgramFingerprint>,
    initialized: bool,
}

impl ReasoningSimilarityDetector {
    pub fn new(config: ReasoningSimilarityConfig) -> Self {
        Self {
            config,
            committed: VecDeque::new(),
            rejected: Vec::new(),
            initialized: false,
        }
    }

    fn remember(&mut self, reasoning: String, window: usize) {
        let fingerprint = self.fingerprint(&reasoning);
        if fingerprint.normalized_char_count() < self.config.min_chars {
            return;
        }

        self.committed.push_back(fingerprint);
        while self.committed.len() > window {
            self.committed.pop_front();
        }
    }

    fn fingerprint(&self, reasoning: &str) -> NgramFingerprint {
        NgramFingerprint::new(reasoning, self.config.ngram_size)
    }
}

impl LoopDetector for ReasoningSimilarityDetector {
    fn initialize(&mut self, messages: &PhiHistory, window: usize) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        for reasoning in messages.iter().filter_map(reasoning_text) {
            self.remember(reasoning, window);
        }
    }

    fn inspect_candidate(&mut self, message: &PhiMessage, _window: usize) -> Option<LoopDetection> {
        let Some(reasoning) = reasoning_text(message) else {
            self.rejected.clear();
            return None;
        };

        let fingerprint = self.fingerprint(&reasoning);
        if fingerprint.normalized_char_count() < self.config.min_chars {
            self.rejected.clear();
            return None;
        }

        let strongest_similarity = self
            .committed
            .iter()
            .chain(self.rejected.iter())
            .map(|previous| {
                fingerprint
                    .dice_similarity(previous)
                    .max(fingerprint.containment_similarity(previous))
            })
            .fold(0.0, f64::max);

        if strongest_similarity < self.config.similarity_threshold {
            self.rejected.clear();
            return None;
        }

        self.rejected.push(fingerprint);
        Some(LoopDetection {
            detector: "reasoning_similarity",
            detail: format!(
                "similarity {strongest_similarity:.3} exceeded threshold {:.3}",
                self.config.similarity_threshold
            ),
        })
    }

    fn commit(&mut self, message: &PhiMessage, window: usize) {
        self.rejected.clear();
        if let Some(reasoning) = reasoning_text(message) {
            self.remember(reasoning, window);
        }
    }
}

fn reasoning_text(message: &PhiMessage) -> Option<String> {
    let PhiMessage::Assistant(assistant) = message else {
        return None;
    };

    let reasoning = assistant
        .reasoning
        .iter()
        .flat_map(|block| block.content.iter())
        .filter_map(PhiReasoningContent::display_text)
        .filter(|text: &&str| !text.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .join("\n");

    (!reasoning.is_empty()).then_some(reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::PhiHistory;
    use crate::message::PhiReasoningContent;

    fn assistant_reasoning(text: &str) -> PhiMessage {
        PhiMessage::reasoning(
            None,
            vec![PhiReasoningContent::Text {
                text: text.to_string(),
                signature: None,
            }],
        )
    }

    fn detector() -> ReasoningSimilarityDetector {
        ReasoningSimilarityDetector::new(ReasoningSimilarityConfig {
            ngram_size: 4,
            similarity_threshold: 0.9,
            min_chars: 10,
        })
    }

    #[test]
    fn detects_repeated_reasoning() {
        let previous = assistant_reasoning("This is a sufficiently long repeated reasoning block.");
        let candidate =
            assistant_reasoning("This is a sufficiently long repeated reasoning block.");
        let mut detector = detector();
        detector.initialize(&PhiHistory::from_messages(vec![previous]), 5);

        assert!(detector.inspect_candidate(&candidate, 5).is_some());
    }

    #[test]
    fn accepts_distinct_reasoning() {
        let previous = assistant_reasoning("This reasoning inspects kernel allocation boundaries.");
        let candidate =
            assistant_reasoning("This completely different analysis checks authentication flow.");
        let mut detector = detector();
        detector.initialize(&PhiHistory::from_messages(vec![previous]), 5);

        assert!(detector.inspect_candidate(&candidate, 5).is_none());
    }

    #[test]
    fn detects_repeated_reasoning_embedded_in_longer_response() {
        let previous = assistant_reasoning(
            "Inspect the allocation, calculate the length, and verify the same bound.",
        );
        let candidate = assistant_reasoning(
            "Start over. Inspect the allocation, calculate the length, and verify the same bound. Then repeat the calculation with more commentary.",
        );
        let mut detector = detector();
        detector.initialize(&PhiHistory::from_messages(vec![previous]), 5);

        assert!(detector.inspect_candidate(&candidate, 5).is_some());
    }
}
