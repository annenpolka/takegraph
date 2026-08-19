//! UTC timestamps for capture commits.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Returns the current UTC time as RFC 3339.
pub trait Clock: Send + Sync {
    /// RFC 3339 UTC timestamp.
    fn now_utc(&self) -> String;
}

/// System clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc(&self) -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
    }
}

/// Fixed timestamp used by tests.
#[derive(Debug, Clone)]
pub struct FixedClock(pub String);

impl Clock for FixedClock {
    fn now_utc(&self) -> String {
        self.0.clone()
    }
}
