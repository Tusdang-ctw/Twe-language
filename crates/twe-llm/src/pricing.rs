//! web3d-M5: what a call cost, from its token usage.
//!
//! Anthropic list prices in US dollars per million tokens, as published
//! 2026-09-25. Cache reads are the published rate where one is listed
//! (Fable 5.1, Opus 5.5, Sonnet 5.5) and the standard 0.1× input rate
//! otherwise. A 5-minute prompt-cache write bills at 1.25× the input
//! price. Models not listed (local ones, unknown ids) cost `None`, not
//! zero, so a missing price is never reported as free.

use crate::Usage;

/// Dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

/// The prices for `model`: an exact id, or an id plus a snapshot date
/// (`claude-haiku-4-5-20251001`).
pub fn prices(model: &str) -> Option<Prices> {
    const TABLE: &[(&str, f64, f64, f64)] = &[
        ("claude-fable-5-1", 10.0, 50.0, 0.25),
        ("claude-fable-5", 10.0, 50.0, 1.0),
        ("claude-opus-5-5", 4.0, 20.0, 0.20),
        ("claude-opus-5", 5.0, 25.0, 0.50),
        ("claude-opus-4-8", 5.0, 25.0, 0.50),
        ("claude-opus-4-7", 5.0, 25.0, 0.50),
        ("claude-opus-4-6", 5.0, 25.0, 0.50),
        ("claude-sonnet-5-5", 2.0, 10.0, 0.20),
        ("claude-sonnet-5", 2.0, 10.0, 0.20),
        ("claude-sonnet-4-6", 3.0, 15.0, 0.30),
        ("claude-haiku-4-5", 1.0, 5.0, 0.10),
    ];
    let family = match model.rsplit_once('-') {
        Some((family, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => family,
        _ => model,
    };
    TABLE
        .iter()
        .find(|(id, ..)| *id == family)
        .map(|&(_, input, output, cache_read)| Prices { input, output, cache_read })
}

/// The cost of `usage` on `model` in US dollars.
pub fn cost(model: &str, usage: Usage) -> Option<f64> {
    let p = prices(model)?;
    let per = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
    Some(
        per(usage.input, p.input)
            + per(usage.output, p.output)
            + per(usage.cache_read, p.cache_read)
            + per(usage.cache_write, p.input * 1.25),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(prices("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(prices("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(prices("claude-haiku-4-5-20251001").unwrap().output, 5.0);
        assert!(prices("claude-opus-5-55").is_none());
        assert!(prices("llama3.1:8b").is_none());
    }

    #[test]
    fn cost_bills_each_kind_of_token() {
        let usage = Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        // 2 + 10 + 0.20 + 2.5
        let c = cost("claude-sonnet-5-5", usage).unwrap();
        assert!((c - 14.7).abs() < 1e-9, "{c}");
        assert_eq!(cost("local-model", usage), None);
    }
}
