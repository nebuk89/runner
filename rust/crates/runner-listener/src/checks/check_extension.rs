// Check extension trait and result type.
// Maps to the C# Runner.Listener/Checks/CheckExtension.cs.

/// Result of a single diagnostic check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// The name of the check (e.g. "Internet Connection").
    pub name: String,
    /// A description of what the check verifies.
    pub description: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Additional detail about the result (e.g. error message).
    pub detail: Option<String>,
    /// The link to the documentation for this check.
    pub doc_url: Option<String>,
}

impl CheckResult {
    /// Create a passing check result.
    pub fn pass(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            passed: true,
            detail: None,
            doc_url: None,
        }
    }

    /// Create a failing check result.
    pub fn fail(
        name: impl Into<String>,
        description: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            passed: false,
            detail: Some(detail.into()),
            doc_url: None,
        }
    }

    /// Attach a documentation URL.
    pub fn with_doc_url(mut self, url: impl Into<String>) -> Self {
        self.doc_url = Some(url.into());
        self
    }
}
