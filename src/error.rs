//! One error type for TV operations: a fixed public message and how it ended.
//!
//! Messages are static so no address, credential, or remote payload can reach
//! an MCP client. Whether an operation timed out is recorded where the deadline
//! expires instead of being inferred from the message text.
use crate::metrics::Outcome;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TvError {
    message: &'static str,
    timed_out: bool,
}

impl TvError {
    pub const fn failure(message: &'static str) -> Self {
        Self {
            message,
            timed_out: false,
        }
    }

    pub const fn timeout(message: &'static str) -> Self {
        Self {
            message,
            timed_out: true,
        }
    }

    pub const fn message(self) -> &'static str {
        self.message
    }

    pub const fn outcome(self) -> Outcome {
        if self.timed_out {
            Outcome::Timeout
        } else {
            Outcome::Failure
        }
    }
}

impl From<&'static str> for TvError {
    fn from(message: &'static str) -> Self {
        Self::failure(message)
    }
}

impl From<TvError> for &'static str {
    fn from(error: TvError) -> Self {
        error.message
    }
}

impl PartialEq<&str> for TvError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl std::fmt::Display for TvError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for TvError {}

/// The metrics outcome of a finished step, for any error that names how it ended.
pub fn outcome<T, E: Copy + Into<TvError>>(result: &Result<T, E>) -> Outcome {
    match result {
        Ok(_) => Outcome::Success,
        Err(error) => (*error).into().outcome(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_comes_from_the_constructor_not_the_message_text() {
        let stale = "TV connection is stale; reconnect and observe again";
        assert_eq!(TvError::timeout(stale).outcome(), Outcome::Timeout);
        assert_eq!(TvError::failure(stale).outcome(), Outcome::Failure);
        assert_eq!(
            TvError::failure("Capture timed out").outcome(),
            Outcome::Failure
        );
        assert_eq!(outcome(&Ok::<(), TvError>(())), Outcome::Success);
        assert_eq!(outcome(&Err::<(), _>("plain failure")), Outcome::Failure);
        assert_eq!(TvError::timeout(stale), stale);
        assert_eq!(<&str>::from(TvError::timeout(stale)), stale);
    }
}
