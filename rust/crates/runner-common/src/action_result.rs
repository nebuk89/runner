// ActionResult enum mapping `ActionResult.cs`.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The result of an action step execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionResult {
    Success = 0,
    Failure = 1,
    Cancelled = 2,
    Skipped = 3,
}

impl fmt::Display for ActionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionResult::Success => write!(f, "Success"),
            ActionResult::Failure => write!(f, "Failure"),
            ActionResult::Cancelled => write!(f, "Cancelled"),
            ActionResult::Skipped => write!(f, "Skipped"),
        }
    }
}

impl ActionResult {
    /// Returns `true` if the result represents a successful outcome.
    pub fn is_success(&self) -> bool {
        matches!(self, ActionResult::Success)
    }
}
