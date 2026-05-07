//! Parser for compound duration strings like `1h30m45s`.
//!
//! Grammar (informal):
//!   duration = component+
//!   component = integer unit
//!   unit = "h" | "m" | "s"
//!
//! Constraints:
//!   - units must appear in order h -> m -> s
//!   - each unit may appear at most once
//!   - integer values only, must be non-negative
//!   - at least one component required
//!   - total duration must be > 0

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("empty duration string")]
    Empty,

    #[error("unexpected character {0:?} at position {1}")]
    UnexpectedChar(char, usize),

    #[error("missing unit suffix after number")]
    MissingUnit,

    #[error("number overflow")]
    Overflow,

    #[error("unit {0:?} appears twice")]
    DuplicateUnit(char),

    #[error("unit {0:?} appears after smaller unit (must be ordered h, m, s)")]
    OutOfOrderUnit(char),

    #[error("duration must be greater than zero")]
    Zero,
}

/// Parse a duration string like `"1h30m"`, `"5m"`, `"30s"`.
pub fn parse(s: &str) -> Result<Duration, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }

    let mut total: u64 = 0;
    let mut current: Option<u64> = None;
    let mut last_unit_rank: i8 = -1; // h=0, m=1, s=2; we require strictly increasing
    let mut seen_h = false;
    let mut seen_m = false;
    let mut seen_s = false;

    for (i, ch) in s.char_indices() {
        if let Some(d) = ch.to_digit(10) {
            let prev = current.unwrap_or(0);
            let next = prev
                .checked_mul(10)
                .and_then(|v| v.checked_add(d as u64))
                .ok_or(ParseError::Overflow)?;
            current = Some(next);
        } else {
            let value = current.take().ok_or(ParseError::UnexpectedChar(ch, i))?;
            let (rank, multiplier, dup_flag) = match ch {
                'h' => (0, 3600, &mut seen_h),
                'm' => (1, 60, &mut seen_m),
                's' => (2, 1, &mut seen_s),
                _ => return Err(ParseError::UnexpectedChar(ch, i)),
            };
            if *dup_flag {
                return Err(ParseError::DuplicateUnit(ch));
            }
            *dup_flag = true;
            if (rank as i8) <= last_unit_rank {
                return Err(ParseError::OutOfOrderUnit(ch));
            }
            last_unit_rank = rank as i8;
            let component = value
                .checked_mul(multiplier)
                .ok_or(ParseError::Overflow)?;
            total = total
                .checked_add(component)
                .ok_or(ParseError::Overflow)?;
        }
    }

    if current.is_some() {
        return Err(ParseError::MissingUnit);
    }
    if total == 0 {
        return Err(ParseError::Zero);
    }
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn single_units() {
        assert_eq!(parse("30s").unwrap(), d(30));
        assert_eq!(parse("5m").unwrap(), d(300));
        assert_eq!(parse("2h").unwrap(), d(7200));
    }

    #[test]
    fn compound() {
        assert_eq!(parse("1h30m").unwrap(), d(5400));
        assert_eq!(parse("2m30s").unwrap(), d(150));
        assert_eq!(parse("1h30m45s").unwrap(), d(5445));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse(""), Err(ParseError::Empty));
    }

    #[test]
    fn rejects_zero() {
        assert_eq!(parse("0s"), Err(ParseError::Zero));
        assert_eq!(parse("0h0m0s"), Err(ParseError::Zero));
    }

    #[test]
    fn rejects_missing_unit() {
        assert_eq!(parse("30"), Err(ParseError::MissingUnit));
        assert_eq!(parse("1h30"), Err(ParseError::MissingUnit));
    }

    #[test]
    fn rejects_unknown_unit() {
        assert!(matches!(parse("5d"), Err(ParseError::UnexpectedChar('d', _))));
    }

    #[test]
    fn rejects_duplicate_unit() {
        assert_eq!(parse("5m5m"), Err(ParseError::DuplicateUnit('m')));
    }

    #[test]
    fn rejects_out_of_order() {
        assert_eq!(parse("30s5m"), Err(ParseError::OutOfOrderUnit('m')));
        assert_eq!(parse("5m1h"), Err(ParseError::OutOfOrderUnit('h')));
        assert_eq!(parse("30s1h"), Err(ParseError::OutOfOrderUnit('h')));
    }

    #[test]
    fn rejects_leading_unit() {
        assert!(matches!(parse("h30"), Err(ParseError::UnexpectedChar('h', 0))));
    }

    #[test]
    fn handles_large_values() {
        // 90 minutes parsed as a single m component should still work
        assert_eq!(parse("90m").unwrap(), d(5400));
    }
}
