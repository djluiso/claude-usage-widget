//! Anthropic's published API prices, used for the $ estimates.
//!
//! Source: https://platform.claude.com/docs/en/about-claude/pricing
//! (checked 2026-11-12). These are list prices for the Claude API; they are
//! not what a Pro/Max subscription bills.

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// 5-minute cache write.
    pub write_5m: f64,
    /// 1-hour cache write (Claude Code uses these).
    pub write_1h: f64,
    /// Cache hits and refreshes.
    pub read: f64,
}

const fn p(input: f64, write_5m: f64, write_1h: f64, read: f64, output: f64) -> Price {
    Price {
        input,
        output,
        write_5m,
        write_1h,
        read,
    }
}

/// Model id (without the `claude-` prefix) -> price. Matched by longest
/// prefix, so dated ids like `claude-sonnet-4-5-20250929` resolve too.
/// Columns follow the pricing page: input, 5m write, 1h write, cache read, output.
pub const TABLE: &[(&str, &str, Price)] = &[
    ("fable-5-2", "Fable 5.2", p(8.0, 10.0, 16.0, 0.20, 40.0)),
    ("mythos-5-2", "Mythos 5.2", p(8.0, 10.0, 16.0, 0.20, 40.0)),
    ("fable-5-1", "Fable 5.1", p(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("mythos-5-1", "Mythos 5.1", p(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("fable-5", "Fable 5", p(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("mythos-5", "Mythos 5", p(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("opus-5-5", "Opus 5.5", p(4.0, 5.0, 8.0, 0.20, 20.0)),
    ("opus-5", "Opus 5", p(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("opus-4-8", "Opus 4.8", p(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("opus-4-7", "Opus 4.7", p(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("opus-4-6", "Opus 4.6", p(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("opus-4-5", "Opus 4.5", p(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("opus-4-1", "Opus 4.1", p(15.0, 18.75, 30.0, 1.50, 75.0)),
    ("opus-4", "Opus 4", p(15.0, 18.75, 30.0, 1.50, 75.0)),
    ("sonnet-5-5", "Sonnet 5.5", p(2.0, 2.5, 4.0, 0.10, 10.0)),
    ("sonnet-5", "Sonnet 5", p(2.0, 2.5, 4.0, 0.20, 10.0)),
    ("sonnet-4-6", "Sonnet 4.6", p(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("sonnet-4-5", "Sonnet 4.5", p(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("sonnet-4", "Sonnet 4", p(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("haiku-5-5", "Haiku 5.5", p(0.10, 0.125, 0.20, 0.01, 0.50)),
    ("haiku-4-5", "Haiku 4.5", p(1.0, 1.25, 2.0, 0.10, 5.0)),
];

/// Haiku 5.5 costs more for prompts over 100K tokens.
const HAIKU_5_5_LONG: Price = p(0.50, 0.625, 1.0, 0.05, 2.50);
const HAIKU_5_5_LONG_ABOVE: u64 = 100_000;

/// Current model of each family, used for ids not in the table
/// (e.g. a model released after this build).
const FAMILY_FALLBACK: &[(&str, &str)] = &[
    ("fable", "fable-5-2"),
    ("mythos", "mythos-5-2"),
    ("opus", "opus-5-5"),
    ("sonnet", "sonnet-5-5"),
    ("haiku", "haiku-5-5"),
];

fn get(key: &str) -> Option<Price> {
    TABLE.iter().find(|(k, ..)| *k == key).map(|(.., p)| *p)
}

/// Price for a model id, or None for ids that aren't Claude models
/// (Claude Code logs some `<synthetic>` messages with no real usage).
pub fn lookup(model: &str) -> Option<Price> {
    let m = model.to_ascii_lowercase();
    let id = m.strip_prefix("claude-").unwrap_or(&m);
    let exact = TABLE
        .iter()
        .filter(|(k, ..)| {
            id.strip_prefix(k)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '@', '[']))
        })
        .max_by_key(|(k, ..)| k.len())
        .map(|(.., p)| *p);
    exact.or_else(|| {
        FAMILY_FALLBACK
            .iter()
            .find(|(fam, _)| id.contains(fam))
            .and_then(|(_, key)| get(key))
    })
}

/// Token counts of one API response.
#[derive(Clone, Copy, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub write_5m: u64,
    pub write_1h: u64,
    pub read: u64,
}

/// Estimated USD cost of one response.
pub fn cost(model: &str, t: &Tokens) -> f64 {
    let Some(mut p) = lookup(model) else {
        return 0.0;
    };
    let prompt = t.input + t.write_5m + t.write_1h + t.read;
    if prompt > HAIKU_5_5_LONG_ABOVE && lookup(model) == get("haiku-5-5") {
        p = HAIKU_5_5_LONG;
    }
    (t.input as f64 * p.input
        + t.output as f64 * p.output
        + t.write_5m as f64 * p.write_5m
        + t.write_1h as f64 * p.write_1h
        + t.read as f64 * p.read)
        / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_dated_ids() {
        assert_eq!(lookup("claude-opus-5-5"), get("opus-5-5"));
        assert_eq!(lookup("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(lookup("claude-opus-5"), get("opus-5"));
        assert_eq!(lookup("claude-sonnet-4-5-20250929"), get("sonnet-4-5"));
        assert_eq!(lookup("claude-opus-4-1-20250805"), get("opus-4-1"));
        assert_eq!(lookup("claude-opus-4-20250514"), get("opus-4"));
        assert_eq!(lookup("claude-opus-4-5@20251101"), get("opus-4-5"));
        assert_eq!(lookup("claude-fable-5-2"), get("fable-5-2"));
        assert_eq!(lookup("claude-opus-5-5[1m]"), get("opus-5-5"));
    }

    #[test]
    fn unknown_ids_fall_back_by_family() {
        assert_eq!(lookup("claude-opus-6"), get("opus-5-5"));
        assert_eq!(lookup("claude-3-5-haiku-20241022"), get("haiku-5-5"));
        assert_eq!(lookup("<synthetic>"), None);
        assert_eq!(cost("<synthetic>", &Tokens::default()), 0.0);
    }

    #[test]
    fn cache_writes_are_priced_by_duration() {
        let t = Tokens {
            input: 1_000_000,
            output: 1_000_000,
            write_5m: 1_000_000,
            write_1h: 1_000_000,
            read: 1_000_000,
        };
        // Opus 5.5: 4 + 20 + 5 + 8 + 0.20
        assert!((cost("claude-opus-5-5", &t) - 37.2).abs() < 1e-9);
    }

    #[test]
    fn haiku_5_5_long_prompts() {
        let short = Tokens {
            input: 100_000,
            output: 1_000_000,
            ..Default::default()
        };
        let long = Tokens {
            input: 100_001,
            ..short
        };
        assert!((cost("claude-haiku-5-5", &short) - (0.01 + 0.5)).abs() < 1e-9);
        assert!((cost("claude-haiku-5-5", &long) - (100_001.0 * 0.5 / 1e6 + 2.5)).abs() < 1e-9);
    }
}
