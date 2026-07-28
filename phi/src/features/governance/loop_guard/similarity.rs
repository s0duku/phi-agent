use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
};

#[derive(Clone, Debug)]
pub struct NgramFingerprint {
    normalized_char_count: usize,
    ngrams: HashSet<u64>,
}

impl NgramFingerprint {
    pub fn new(input: &str, ngram_size: usize) -> Self {
        let normalized = normalize(input);
        Self {
            normalized_char_count: normalized.chars().count(),
            ngrams: ngram_hashes(&normalized, ngram_size),
        }
    }

    pub fn normalized_char_count(&self) -> usize {
        self.normalized_char_count
    }

    pub fn dice_similarity(&self, other: &Self) -> f64 {
        if self.ngrams.is_empty() || other.ngrams.is_empty() {
            return f64::from(self.ngrams == other.ngrams);
        }

        let intersection = self.ngrams.intersection(&other.ngrams).count();
        2.0 * intersection as f64 / (self.ngrams.len() + other.ngrams.len()) as f64
    }

    pub fn containment_similarity(&self, other: &Self) -> f64 {
        if self.ngrams.is_empty() || other.ngrams.is_empty() {
            return f64::from(self.ngrams == other.ngrams);
        }

        let intersection = self.ngrams.intersection(&other.ngrams).count();
        intersection as f64 / self.ngrams.len().min(other.ngrams.len()) as f64
    }
}

fn normalize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut pending_space = false;
    let mut in_number = false;

    for character in input.chars().flat_map(char::to_lowercase) {
        if character.is_whitespace() {
            pending_space = !output.is_empty();
            in_number = false;
            continue;
        }

        if pending_space {
            output.push(' ');
            pending_space = false;
        }

        if character.is_numeric() {
            if !in_number {
                output.push_str("<num>");
                in_number = true;
            }
        } else {
            output.push(character);
            in_number = false;
        }
    }

    output
}

fn ngram_hashes(input: &str, ngram_size: usize) -> HashSet<u64> {
    if ngram_size == 0 {
        return HashSet::new();
    }

    let characters = input.chars().collect::<Vec<_>>();
    if characters.len() < ngram_size {
        return HashSet::new();
    }

    characters
        .windows(ngram_size)
        .map(|window| {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            hasher.finish()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_ignores_whitespace_case_and_number_changes() {
        let left = NgramFingerprint::new("Check FILE 123, then inspect result 456.", 4);
        let right = NgramFingerprint::new("check   file 999, then inspect result 1000.", 4);

        assert_eq!(left.dice_similarity(&right), 1.0);
    }

    #[test]
    fn fingerprint_caches_normalized_character_count() {
        let fingerprint = NgramFingerprint::new("  A  123  B  ", 4);

        assert_eq!(fingerprint.normalized_char_count(), "a <num> b".len());
    }

    #[test]
    fn unrelated_text_has_low_similarity() {
        let left = NgramFingerprint::new("Inspect the kernel allocation and bounds check.", 4);
        let right =
            NgramFingerprint::new("Render the frontend dashboard and update its colors.", 4);

        assert_eq!(
            left.normalized_char_count(),
            "inspect the kernel allocation and bounds check.".len()
        );
        assert!(left.dice_similarity(&right) < 0.2);
        assert!(left.containment_similarity(&right) < 0.2);
    }

    #[test]
    fn containment_detects_repeated_reasoning_with_extra_text() {
        let repeated = NgramFingerprint::new(
            "Inspect the allocation, calculate the length, and verify the same bound.",
            4,
        );
        let expanded = NgramFingerprint::new(
            "Start over. Inspect the allocation, calculate the length, and verify the same bound. Then repeat the calculation with more commentary.",
            4,
        );

        assert!(repeated.dice_similarity(&expanded) < repeated.containment_similarity(&expanded));
        assert!(repeated.containment_similarity(&expanded) > 0.95);
    }
}
