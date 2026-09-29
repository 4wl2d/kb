//! Budget measurement (docs/architecture.md §5.1).
//!
//! * `bytes`: exact UTF-8 bytes of each rendered piece. For `compact` and `human` output the
//!   pieces concatenate to exactly the printed payload, so the sum is the exact payload size.
//!   For `json` each piece is rendered at its final indentation inside the pretty-printed CLI
//!   envelope, separators and brackets included (see `present`), so the sum is exactly the
//!   size of the printed `result` member; the envelope around it is not counted.
//! * `tokens-est`: `ceil(ascii_bytes / 4) + ceil(non_ascii_chars / 2)` per piece, summed.
//!   Summing per piece upper-bounds the whole-text estimate. It is a deterministic estimate,
//!   not a tokenizer count.

use crate::model::BudgetUnit;

/// Deterministic token estimate of one piece of text.
pub fn estimate_tokens(text: &str) -> u64 {
    let (mut ascii, mut other) = (0u64, 0u64);
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    ascii.div_ceil(4) + other.div_ceil(2)
}

/// Cost of one rendered piece in the given unit.
pub fn measure(text: &str, unit: BudgetUnit) -> u64 {
    match unit {
        BudgetUnit::Bytes => text.len() as u64,
        BudgetUnit::TokensEst => estimate_tokens(text),
    }
}

/// Smallest `r` with `f(r) <= r`, for `f` monotone non-decreasing in `r` whose growth comes
/// only from the rendered width of `r` (so iteration converges in a few steps).
pub(super) fn least_fit(f: impl Fn(u64) -> u64) -> u64 {
    let mut r = f(0);
    for _ in 0..64 {
        let next = f(r);
        if next <= r {
            return r;
        }
        r = next;
    }
    r
}

/// Fixed point `u = f(u)` reached by iterating downward from `f(start)`, for `f` monotone
/// non-decreasing with `f(start) <= start`.
pub(super) fn settle(f: impl Fn(u64) -> u64, start: u64) -> u64 {
    let mut u = f(start);
    for _ in 0..64 {
        let next = f(u);
        if next >= u {
            return u.max(next);
        }
        u = next;
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("ток"), 2);
        assert_eq!(estimate_tokens("ab ток"), 1 + 2);
        assert_eq!(measure("ток", BudgetUnit::Bytes), 6);
    }

    #[test]
    fn per_piece_sum_upper_bounds_whole_text() {
        let pieces = ["abc", "de", "жзи", "k"];
        let sum: u64 = pieces.iter().map(|p| estimate_tokens(p)).sum();
        assert!(sum >= estimate_tokens(&pieces.concat()));
    }

    #[test]
    fn fixed_points() {
        let width = |n: u64| n.to_string().len() as u64;
        // required budget: base 95 plus the width of the printed budget
        let r = least_fit(|r| 95 + width(r));
        assert_eq!(r, 97);
        assert!(95 + width(r) <= r && 95 + width(r - 1) > r - 1);
        let u = settle(|u| 7 + width(u), 1000);
        assert_eq!(u, 8);
    }
}
