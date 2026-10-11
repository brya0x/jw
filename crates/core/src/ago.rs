//! How long ago a unix time was, in words, for lists of devices and sessions.

/// `then` as an age at `now`, both in unix seconds: "just now", "2 min ago",
/// "3 h ago", "4 d ago".
pub fn ago(now: u64, then: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn times_read_as_ages() {
        assert_eq!(ago(1000, 1000), "just now");
        assert_eq!(ago(1000, 1000 - 125), "2 min ago");
        assert_eq!(ago(100_000, 100_000 - 7300), "2 h ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 200_000), "2 d ago");
        // A clock that went back is not a negative age.
        assert_eq!(ago(10, 100), "just now");
    }
}
