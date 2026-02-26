// TaskResultUtil mapping `Util/TaskResultUtil.cs`.
// Conversion between TaskResult and process return codes, plus result merging.

use crate::action_result::ActionResult;

/// Offset added to TaskResult values to produce process return codes.
const RETURN_CODE_OFFSET: i32 = 100;

/// Task result enum mirroring the C# `TaskResult` from the distributed task pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum TaskResult {
    Succeeded = 0,
    SucceededWithIssues = 1,
    Failed = 2,
    Canceled = 3,
    Skipped = 4,
    Abandoned = 5,
}

impl TaskResult {
    /// Create a `TaskResult` from its integer representation.
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(TaskResult::Succeeded),
            1 => Some(TaskResult::SucceededWithIssues),
            2 => Some(TaskResult::Failed),
            3 => Some(TaskResult::Canceled),
            4 => Some(TaskResult::Skipped),
            5 => Some(TaskResult::Abandoned),
            _ => None,
        }
    }

    /// Convert to `ActionResult`.
    pub fn to_action_result(self) -> ActionResult {
        match self {
            TaskResult::Succeeded | TaskResult::SucceededWithIssues => ActionResult::Success,
            TaskResult::Failed => ActionResult::Failure,
            TaskResult::Canceled => ActionResult::Cancelled,
            TaskResult::Skipped => ActionResult::Skipped,
            TaskResult::Abandoned => ActionResult::Failure,
        }
    }
}

impl std::fmt::Display for TaskResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskResult::Succeeded => write!(f, "Succeeded"),
            TaskResult::SucceededWithIssues => write!(f, "SucceededWithIssues"),
            TaskResult::Failed => write!(f, "Failed"),
            TaskResult::Canceled => write!(f, "Canceled"),
            TaskResult::Skipped => write!(f, "Skipped"),
            TaskResult::Abandoned => write!(f, "Abandoned"),
        }
    }
}

/// Task result utilities.
pub struct TaskResultUtil;

impl TaskResultUtil {
    /// Check whether a return code can be translated to a valid `TaskResult`.
    pub fn is_valid_return_code(return_code: i32) -> bool {
        let result_int = return_code - RETURN_CODE_OFFSET;
        TaskResult::from_i32(result_int).is_some()
    }

    /// Translate a `TaskResult` to a process return code.
    pub fn translate_to_return_code(result: TaskResult) -> i32 {
        RETURN_CODE_OFFSET + (result as i32)
    }

    /// Translate a process return code to a `TaskResult`.
    ///
    /// Returns `TaskResult::Failed` for unrecognized codes.
    pub fn translate_from_return_code(return_code: i32) -> TaskResult {
        let result_int = return_code - RETURN_CODE_OFFSET;
        TaskResult::from_i32(result_int).unwrap_or(TaskResult::Failed)
    }

    /// Merge two task results, keeping the "worst" (highest severity) result.
    ///
    /// Result precedence (worst to best):
    /// - `Abandoned`, `Skipped`, `Canceled`, `Failed`, `SucceededWithIssues`, `Succeeded`
    ///
    /// If `current_result` is `None`, the `coming_result` is returned.
    pub fn merge_task_results(
        current_result: Option<TaskResult>,
        coming_result: TaskResult,
    ) -> TaskResult {
        match current_result {
            None => coming_result,
            Some(current) => {
                // If current is worse than Failed (Canceled/Skipped/Abandoned),
                // keep current
                if current > TaskResult::Failed {
                    return current;
                }

                // If coming result is worse than or equal to current, use coming
                if coming_result >= current {
                    return coming_result;
                }

                current
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_to_return_code() {
        assert_eq!(TaskResultUtil::translate_to_return_code(TaskResult::Succeeded), 100);
        assert_eq!(TaskResultUtil::translate_to_return_code(TaskResult::Failed), 102);
        assert_eq!(TaskResultUtil::translate_to_return_code(TaskResult::Canceled), 103);
    }

    #[test]
    fn test_translate_from_return_code() {
        assert_eq!(
            TaskResultUtil::translate_from_return_code(100),
            TaskResult::Succeeded
        );
        assert_eq!(
            TaskResultUtil::translate_from_return_code(102),
            TaskResult::Failed
        );
        assert_eq!(
            TaskResultUtil::translate_from_return_code(999),
            TaskResult::Failed
        );
    }

    #[test]
    fn test_is_valid_return_code() {
        assert!(TaskResultUtil::is_valid_return_code(100)); // Succeeded
        assert!(TaskResultUtil::is_valid_return_code(105)); // Abandoned
        assert!(!TaskResultUtil::is_valid_return_code(99));
        assert!(!TaskResultUtil::is_valid_return_code(106));
    }

    #[test]
    fn test_merge_task_results_none() {
        assert_eq!(
            TaskResultUtil::merge_task_results(None, TaskResult::Succeeded),
            TaskResult::Succeeded
        );
    }

    #[test]
    fn test_merge_task_results_worse_result() {
        assert_eq!(
            TaskResultUtil::merge_task_results(Some(TaskResult::Succeeded), TaskResult::Failed),
            TaskResult::Failed
        );
    }

    #[test]
    fn test_merge_task_results_keep_current_if_canceled() {
        assert_eq!(
            TaskResultUtil::merge_task_results(Some(TaskResult::Canceled), TaskResult::Failed),
            TaskResult::Canceled
        );
    }

    #[test]
    fn test_task_result_to_action_result() {
        assert_eq!(TaskResult::Succeeded.to_action_result(), ActionResult::Success);
        assert_eq!(TaskResult::Failed.to_action_result(), ActionResult::Failure);
        assert_eq!(TaskResult::Canceled.to_action_result(), ActionResult::Cancelled);
        assert_eq!(TaskResult::Skipped.to_action_result(), ActionResult::Skipped);
    }
}
