//! Switch budget: every wait is bounded twice (its own timeout + the total
//! budget). A switch that runs out of budget fails and rolls back instead of
//! holding the lock; the LuCI request that started it has already returned.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::system::clock::{fmt_cs, now_cs};

static DEADLINE_CS: AtomicU64 = AtomicU64::new(0);

/// `budget_start`: set the switch deadline (centiseconds).
pub fn budget_start(budget_s: u32) {
    let now = now_cs();
    let deadline = now.saturating_add(budget_s as u64 * 100);
    DEADLINE_CS.store(deadline, Ordering::Relaxed);
}

/// Disable the budget (a test seam / "no usable clock" path).
pub fn budget_clear() {
    DEADLINE_CS.store(0, Ordering::Relaxed);
}

/// `budget_left_cs`.
pub fn budget_left_cs() -> u64 {
    let deadline = DEADLINE_CS.load(Ordering::Relaxed);
    if deadline == 0 {
        return u64::MAX;
    }
    let now = now_cs();
    if now == 0 {
        return u64::MAX;
    }
    deadline.saturating_sub(now)
}

/// `budget_expired`: no time left (in centiseconds).
pub fn budget_left() -> bool {
    budget_left_cs() > 0
}

/// `bounded_sleep <seconds>`: sleep in one-second steps, never longer than the
/// remaining budget. Returns false when the budget was already gone.
pub fn bounded_sleep(seconds: u32) -> bool {
    let left_s = budget_left_cs() / 100;
    if left_s == 0 {
        return false;
    }
    let want = seconds.min(left_s as u32);
    std::thread::sleep(std::time::Duration::from_secs(want as u64));
    true
}

/// Helper for log lines: elapsed centiseconds as a string.
pub fn elapsed_cs(start_cs: u64) -> String {
    fmt_cs(now_cs().saturating_sub(start_cs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_arithmetic() {
        budget_clear();
        assert_eq!(budget_left_cs(), u64::MAX);
        budget_start(60);
        assert!(budget_left());
        budget_clear();
    }
}
