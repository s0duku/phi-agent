pub const APPROX_BYTES_PER_TOKEN: usize = 4;

pub fn approx_token_count_from_bytes(bytes: usize) -> usize {
    if bytes == 0 {
        return 0;
    }
    bytes.div_ceil(APPROX_BYTES_PER_TOKEN)
}

pub fn approx_token_count(text: &str) -> usize {
    approx_token_count_from_bytes(text.len())
}

pub fn approx_token_count_for_strings<'a>(values: impl IntoIterator<Item = &'a str>) -> usize {
    values.into_iter().map(approx_token_count).sum()
}

#[cfg(test)]
mod tests {
    use super::{
        APPROX_BYTES_PER_TOKEN, approx_token_count, approx_token_count_for_strings,
        approx_token_count_from_bytes,
    };

    #[test]
    fn approx_token_count_handles_empty_inputs() {
        assert_eq!(approx_token_count_from_bytes(0), 0);
        assert_eq!(approx_token_count(""), 0);
        assert_eq!(approx_token_count_for_strings([]), 0);
    }

    #[test]
    fn approx_token_count_rounds_up_partial_tokens() {
        assert_eq!(approx_token_count_from_bytes(1), 1);
        assert_eq!(approx_token_count_from_bytes(APPROX_BYTES_PER_TOKEN), 1);
        assert_eq!(approx_token_count_from_bytes(APPROX_BYTES_PER_TOKEN + 1), 2);
    }

    #[test]
    fn approx_token_count_sums_multiple_strings() {
        assert_eq!(approx_token_count_for_strings(["abcd", "ef"]), 2);
    }
}
