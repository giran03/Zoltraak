//! Discord's attachment size ceiling, and the budget a transcode aims at.
//!
//! The limit belongs to the **server**, not the uploader: a bot or webhook
//! posting into an unboosted guild gets 10 MiB no matter whose Nitro paid for
//! anything. Bot mode can read the guild's boost tier and know the real
//! number; webhook mode cannot — a webhook token may not read guild metadata —
//! so it uses the configured preparation budget and corrects itself the first time
//! Discord actually rejects an upload.

const MIB: u64 = 1024 * 1024;

/// Ceilings by guild boost tier. Tiers 0 and 1 share the base limit.
pub const TIER_BASE: u64 = 10 * MIB;
pub const TIER_2: u64 = 50 * MIB;
pub const TIER_3: u64 = 100 * MIB;

/// The ceiling a guild's `premium_tier` buys. Unknown tiers fall back to the
/// base rather than guessing upward — guessing high fails the upload, guessing
/// low only costs some quality.
pub fn for_premium_tier(tier: u8) -> u64 {
    match tier {
        2 => TIER_2,
        3 => TIER_3,
        _ => TIER_BASE,
    }
}

/// How many files Discord will carry on one message. Its own hard limit, not
/// a policy of ours — an eleventh attachment is rejected outright.
pub const MAX_ATTACHMENTS: usize = 10;

/// Overhead reserved on every upload: multipart framing, the boundary, the
/// `payload_json` part, and the headers. 64 KiB is far more than any of that
/// needs, and being a little under the ceiling costs nothing.
const OVERHEAD: u64 = 64 * 1024;

/// How large the file itself may be, given a ceiling of `limit`.
///
/// Deliberately conservative: Discord counts the whole request, and a file
/// that lands one byte over is a wasted transcode plus a wasted upload.
pub fn budget(limit: u64) -> u64 {
    let after_margin = limit.saturating_mul(95) / 100;
    // The floor keeps a nonsensically small ceiling from producing a zero
    // budget, but it can never exceed the ceiling it is derived from — that
    // would wave through files the server is certain to reject.
    after_margin.saturating_sub(OVERHEAD).max(MIB / 4).min(after_margin)
}

/// A new ceiling to remember after Discord rejected `rejected_bytes` as too
/// large. The rejection proves the real limit is below that size, so drop to
/// the largest standard tier that is, and never upward.
pub fn learn_from_rejection(current: u64, rejected_bytes: u64) -> u64 {
    let below = [TIER_3, TIER_2, TIER_BASE].into_iter().find(|tier| *tier < rejected_bytes).unwrap_or(TIER_BASE);
    below.min(current)
}

/// A size as a person would write it. Used in delivery messages and in the
/// settings readout, so it stays in one place.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_tiers_map_to_ceilings() {
        assert_eq!(for_premium_tier(0), TIER_BASE);
        assert_eq!(for_premium_tier(1), TIER_BASE, "one boost buys no extra room");
        assert_eq!(for_premium_tier(2), TIER_2);
        assert_eq!(for_premium_tier(3), TIER_3);
        assert_eq!(for_premium_tier(9), TIER_BASE, "an unknown tier guesses low, never high");
    }

    #[test]
    fn the_budget_stays_under_the_ceiling() {
        assert!(budget(TIER_BASE) < TIER_BASE);
        assert!(budget(TIER_3) < TIER_3);
        // Still most of the allowance — the margin must not be punitive.
        assert!(budget(TIER_BASE) > TIER_BASE * 9 / 10);
    }

    #[test]
    fn a_tiny_ceiling_still_leaves_something_to_aim_at_but_never_more_than_itself() {
        assert!(budget(1024) > 0, "a nonsensical limit must not produce a zero budget");
        for limit in [1, 1024, 64 * 1024, MIB, TIER_BASE, TIER_3] {
            assert!(budget(limit) <= limit, "budget({limit}) must not exceed the ceiling it came from");
        }
    }

    #[test]
    fn a_rejection_lowers_the_ceiling_and_never_raises_it() {
        // Told 50 MiB, rejected at 12 MiB: the guild is really on the base tier.
        assert_eq!(learn_from_rejection(TIER_2, 12 * MIB), TIER_BASE);
        // Told 100 MiB, rejected at 60 MiB: tier 2 is the best that still fits.
        assert_eq!(learn_from_rejection(TIER_3, 60 * MIB), TIER_2);
        // A rejection can never be evidence for a *higher* limit.
        assert_eq!(learn_from_rejection(TIER_BASE, 80 * MIB), TIER_BASE);
    }

    #[test]
    fn sizes_read_the_way_a_person_writes_them() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(10 * MIB), "10.0 MiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(700 * MIB), "700 MiB");
    }
}
