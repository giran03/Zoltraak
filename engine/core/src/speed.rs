//! Throughput measurement and bandwidth limiting.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Sliding-window throughput estimate.
///
/// Bytes are accumulated into short buckets and the rate is the bytes inside
/// the window divided by the time that window actually spans. The result
/// depends on wall-clock time, not on how often samples arrive: several
/// connections delivering chunks microseconds apart can no longer register as
/// an enormous instantaneous rate, which is what made a per-sample estimate
/// spike and sag even over a steady transfer.
#[derive(Debug)]
pub struct SpeedMeter {
    window: Duration,
    /// `(bucket start, bytes)`, oldest first.
    buckets: VecDeque<(Instant, u64)>,
    last_sample: Option<Instant>,
}

/// Width of one accumulation bucket.
const BUCKET: Duration = Duration::from_millis(250);
/// Shortest span a rate is divided over, so the first samples of a transfer
/// ramp up instead of reading as a burst.
const MIN_SPAN: Duration = Duration::from_millis(500);

impl Default for SpeedMeter {
    fn default() -> Self {
        Self::new(Duration::from_secs(4))
    }
}

impl SpeedMeter {
    /// `window` is how much recent history the rate averages over — longer is
    /// steadier, shorter reacts faster to a real change.
    pub fn new(window: Duration) -> Self {
        Self {
            window: window.max(BUCKET * 2),
            buckets: VecDeque::new(),
            last_sample: None,
        }
    }

    pub fn record(&mut self, bytes: u64, now: Instant) {
        if self.last_sample.is_none() {
            // The first sample only starts the clock; it has no interval to
            // measure against.
            self.buckets.push_back((now, 0));
        } else {
            match self.buckets.back_mut() {
                Some((start, total)) if now.saturating_duration_since(*start) < BUCKET => {
                    *total += bytes;
                }
                _ => self.buckets.push_back((now, bytes)),
            }
        }
        self.last_sample = Some(now);

        while let Some(&(start, _)) = self.buckets.front() {
            if self.buckets.len() > 1 && now.saturating_duration_since(start) > self.window {
                self.buckets.pop_front();
            } else {
                break;
            }
        }
    }

    /// Rate as of the most recent sample.
    pub fn bytes_per_second(&self) -> u64 {
        let (Some(&(first, _)), Some(last)) = (self.buckets.front(), self.last_sample) else {
            return 0;
        };
        let total: u64 = self.buckets.iter().map(|&(_, b)| b).sum();
        let span = last.saturating_duration_since(first).max(MIN_SPAN);
        (total as f64 / span.as_secs_f64()) as u64
    }
}

/// A [`RateLimit`] shared by every consumer that must add up to one cap.
///
/// An async mutex, and callers are expected to hold it *across* the throttling
/// sleep rather than only across [`RateLimit::delay_for`] — see that method's
/// documentation for why releasing early lets N consumers reach N times the
/// configured limit.
pub type SharedRateLimit = std::sync::Arc<tokio::sync::Mutex<RateLimit>>;

/// Token-bucket rate limit shared across a download's connections.
#[derive(Debug)]
pub struct RateLimit {
    limit: Option<u64>,
    allowance: f64,
    last_refill: Instant,
}

impl RateLimit {
    /// `None` means unlimited.
    pub fn new(bytes_per_second: Option<u64>, now: Instant) -> Self {
        Self {
            limit: bytes_per_second.filter(|&n| n > 0),
            allowance: bytes_per_second.unwrap_or(0) as f64,
            last_refill: now,
        }
    }

    pub fn is_unlimited(&self) -> bool {
        self.limit.is_none()
    }

    /// Change the limit live. `None`/`Some(0)` lifts it entirely.
    ///
    /// The allowance is reset to a full bucket at the *new* limit rather than
    /// carried over: carrying a large allowance from "unlimited" into a tight
    /// new limit would let one burst ignore the change, and clamping a small
    /// allowance up would let the opposite change grant free bandwidth.
    pub fn set_limit(&mut self, bytes_per_second: Option<u64>, now: Instant) {
        self.limit = bytes_per_second.filter(|&n| n > 0);
        self.allowance = self.limit.unwrap_or(0) as f64;
        self.last_refill = now;
    }

    /// How long to sleep before sending `bytes`, or [`Duration::ZERO`] when the
    /// bucket already has room.
    ///
    /// Callers are expected to hold whatever lock guards this instance across
    /// the returned sleep, not just across this call. Releasing it early lets
    /// a second concurrent caller compute its own delay from the same
    /// near-zero elapsed time the first caller just measured — double-
    /// granting the same accrual window to both, which lets N concurrent
    /// callers add up to N times the configured limit instead of sharing it.
    pub fn delay_for(&mut self, bytes: u64, now: Instant) -> Duration {
        let Some(limit) = self.limit else {
            return Duration::ZERO;
        };

        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.allowance = (self.allowance + elapsed * limit as f64).min(limit as f64);

        if self.allowance >= bytes as f64 {
            self.allowance -= bytes as f64;
            Duration::ZERO
        } else {
            let deficit = bytes as f64 - self.allowance;
            self.allowance = 0.0;
            let wait = Duration::from_secs_f64(deficit / limit as f64);
            // The debt is repaid at `now + wait`, not at `now` — advancing the
            // clock to that virtual instant is what stops the next caller
            // from re-accruing the same window this claim already spent.
            self.last_refill = now + wait;
            wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sample_does_not_produce_a_rate() {
        let mut m = SpeedMeter::default();
        m.record(1000, Instant::now());
        assert_eq!(m.bytes_per_second(), 0);
    }

    #[test]
    fn meter_converges_toward_the_real_rate() {
        let mut m = SpeedMeter::new(Duration::from_secs(4));
        let start = Instant::now();
        // 1000 bytes every 100ms == 10_000 B/s.
        for i in 0..80 {
            m.record(1000, start + Duration::from_millis(100 * i));
        }
        let rate = m.bytes_per_second();
        assert!(
            (9_000..=11_000).contains(&rate),
            "expected ~10000 B/s, got {rate}"
        );
    }

    /// Several connections delivering chunks back-to-back must not read as a
    /// burst: the same bytes per second arriving in clumps has to give the
    /// same rate as arriving evenly.
    #[test]
    fn clumped_arrivals_do_not_spike_the_rate() {
        let mut m = SpeedMeter::default();
        let start = Instant::now();
        let mut peak = 0u64;
        // 8 chunks of 16 KiB, 20µs apart, once every 100ms == ~1.28 MB/s.
        for tick in 0..100u64 {
            for k in 0..8u64 {
                let at = start + Duration::from_millis(100 * tick) + Duration::from_micros(20 * k);
                m.record(16 * 1024, at);
                if tick >= 20 {
                    peak = peak.max(m.bytes_per_second());
                }
            }
        }
        let expected = 8.0 * 16.0 * 1024.0 * 10.0;
        assert!(
            (peak as f64) < expected * 1.25,
            "peak {peak} B/s spiked far above the real {expected} B/s"
        );
        let rate = m.bytes_per_second() as f64;
        assert!((rate - expected).abs() < expected * 0.1, "rate {rate} vs {expected}");
    }

    #[test]
    fn rate_drops_after_a_slowdown() {
        let mut m = SpeedMeter::new(Duration::from_secs(4));
        let start = Instant::now();
        for i in 0..50 {
            m.record(10_000, start + Duration::from_millis(100 * i));
        }
        // Throughput falls to a tenth of that.
        for i in 50..150 {
            m.record(1_000, start + Duration::from_millis(100 * i));
        }
        let rate = m.bytes_per_second();
        assert!((9_000..=11_000).contains(&rate), "expected ~10000 B/s, got {rate}");
    }

    #[test]
    fn unlimited_never_delays() {
        let now = Instant::now();
        let mut r = RateLimit::new(None, now);
        assert!(r.is_unlimited());
        assert_eq!(r.delay_for(10_000_000, now), Duration::ZERO);
    }

    #[test]
    fn zero_limit_is_treated_as_unlimited() {
        // Otherwise a 0 B/s setting would deadlock every download.
        let now = Instant::now();
        let mut r = RateLimit::new(Some(0), now);
        assert!(r.is_unlimited());
        assert_eq!(r.delay_for(1024, now), Duration::ZERO);
    }

    #[test]
    fn burst_within_budget_is_free() {
        let now = Instant::now();
        let mut r = RateLimit::new(Some(1000), now);
        assert_eq!(r.delay_for(1000, now), Duration::ZERO);
    }

    #[test]
    fn exceeding_the_budget_delays_proportionally() {
        let now = Instant::now();
        let mut r = RateLimit::new(Some(1000), now);
        r.delay_for(1000, now); // drain the bucket
        let delay = r.delay_for(500, now);
        assert!(
            (delay.as_secs_f64() - 0.5).abs() < 0.01,
            "expected ~0.5s, got {delay:?}"
        );
    }

    #[test]
    fn bucket_refills_over_time() {
        let now = Instant::now();
        let mut r = RateLimit::new(Some(1000), now);
        r.delay_for(1000, now);
        // A full second later the bucket is full again.
        assert_eq!(r.delay_for(1000, now + Duration::from_secs(1)), Duration::ZERO);
    }

    #[test]
    fn set_limit_starts_the_new_limit_with_a_full_bucket() {
        let now = Instant::now();
        let mut r = RateLimit::new(Some(100), now);
        r.delay_for(100, now); // drain it
        r.set_limit(Some(500), now);
        assert_eq!(r.delay_for(500, now), Duration::ZERO, "the new limit should start fresh");
    }

    /// Simulates the download path's "hold the lock across the sleep"
    /// pattern by advancing the clock by exactly the returned delay between
    /// calls. Over many claims, the one-time free burst from starting with a
    /// full bucket amortizes away, and the effective rate must converge to
    /// the configured cap rather than staying inflated — which is exactly
    /// what would happen if concurrent-looking claims double-granted the
    /// same accrual window instead of compounding.
    #[test]
    fn repeated_deficits_compound_instead_of_overlapping() {
        let start = Instant::now();
        let mut r = RateLimit::new(Some(40_000), start);
        let mut now = start;
        let mut total_bytes = 0u64;

        for _ in 0..40 {
            let delay = r.delay_for(50_000, now);
            now += delay;
            total_bytes += 50_000;
        }

        let elapsed = now.duration_since(start).as_secs_f64();
        let effective_rate = total_bytes as f64 / elapsed;
        assert!(
            effective_rate <= 40_000.0 * 1.05,
            "effective rate {effective_rate} B/s exceeded the 40000 B/s cap — \
             concurrent-style claims must not double-grant the same accrual window"
        );
    }

    #[test]
    fn set_limit_to_none_lifts_it_immediately() {
        let now = Instant::now();
        let mut r = RateLimit::new(Some(10), now);
        r.set_limit(None, now);
        assert!(r.is_unlimited());
        assert_eq!(r.delay_for(1_000_000, now), Duration::ZERO);
    }
}
