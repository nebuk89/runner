// EncodingUtil mapping `Util/EncodingUtil.cs`.
// Encoding/character set helpers.

use crate::host_context::HostContext;
use std::sync::Arc;

/// Encoding utility helpers.
pub struct EncodingUtil;

impl EncodingUtil {
    /// Set the console encoding to UTF-8.
    ///
    /// On Windows this runs `chcp 65001`. On Unix this is a no-op since
    /// UTF-8 is the default encoding.
    pub async fn set_encoding(
        context: &Arc<HostContext>,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        #[cfg(target_os = "windows")]
        {
            use crate::process_invoker::ProcessInvokerService;

            let trace = context.get_trace("EncodingUtil");
            let mut invoker = ProcessInvokerService::new();
            invoker.initialize(context.clone());

            let work_dir = context
                .get_directory(crate::constants::WellKnownDirectory::Work)
                .to_string_lossy()
                .to_string();

            // Try to find chcp
            if let Some(chcp_path) = runner_sdk::WhichUtil::which("chcp", false, None) {
                match invoker
                    .execute(
                        &work_dir,
                        &chcp_path,
                        "65001",
                        None,
                        false,
                        false,
                        cancellation_token,
                    )
                    .await
                {
                    Ok(exit_code) => {
                        if exit_code == 0 {
                            trace.info("Successfully returned to code page 65001 (UTF8)");
                        } else {
                            trace.warning(&format!(
                                "'chcp 65001' failed with exit code {}",
                                exit_code
                            ));
                        }
                    }
                    Err(e) => {
                        trace.warning(&format!("'chcp 65001' failed with exception: {}", e));
                    }
                }
            }
        }

        // On non-Windows platforms, UTF-8 is already the default
        #[cfg(not(target_os = "windows"))]
        {
            let _ = context;
            let _ = cancellation_token;
        }
    }
}
