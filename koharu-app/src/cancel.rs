//! Typed cancellation, so callers can tell "the user cancelled this job" from
//! a real failure without matching on error text.

use std::fmt;

/// Returned (inside `anyhow::Error`) when a long-running job stops because
/// its cancel flag was set. Test for it with [`is_cancelled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Whether `err` is a [`Cancelled`], including when context was added on top.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, anyhow};

    use super::*;

    #[test]
    fn detects_cancelled_through_context() {
        let err = anyhow::Error::from(Cancelled);
        assert!(is_cancelled(&err));

        let wrapped: anyhow::Result<()> = Err(Cancelled.into());
        let err = wrapped.context("page 3").context("pipeline").unwrap_err();
        assert!(is_cancelled(&err));
    }

    #[test]
    fn other_errors_mentioning_cancelled_are_not_cancellation() {
        assert!(!is_cancelled(&anyhow!("upstream request was cancelled")));
    }
}
