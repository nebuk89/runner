// FileContainerServer – HTTP client for the file container service.
//
// Maps `FileContainerServer.cs` from `Runner.Plugins.Artifact`.
// Handles parallel upload / download of artifact files in chunks with retry.

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use runner_sdk::TraceWriter;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Maximum number of retries for a single file operation.
const MAX_RETRIES: u32 = 3;

/// Maximum concurrent uploads (matches C# cap of 2).
const MAX_CONCURRENT_UPLOADS: usize = 2;

// ---------------------------------------------------------------------------
// Container item types returned by the file container REST API
// ---------------------------------------------------------------------------

/// The type of item inside a file container (folder or file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContainerItemType {
    /// A directory entry.
    Folder,
    /// A file entry.
    File,
}

/// A single item (file or folder) in a file container.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContainerItem {
    /// The path of the item inside the container.
    pub path: String,

    /// Whether the item is a file or folder.
    pub item_type: ContainerItemType,

    /// Size of the file in bytes (0 for folders or empty files).
    #[serde(default)]
    pub file_length: i64,
}

/// Wrapper around the File Container REST API.
#[derive(Debug, Clone)]
pub struct FileContainerServer {
    client: Client,
    base_url: String,
    auth_token: String,
    #[allow(dead_code)]
    project_id: Uuid,
    container_id: i64,
    container_path: String,
}

/// Holds the results of a parallel upload operation.
#[derive(Debug, Default)]
struct UploadResult {
    retry_files: Vec<String>,
    total_size_uploaded: i64,
}

impl UploadResult {
    #[allow(dead_code)]
    fn merge(&mut self, other: UploadResult) {
        self.retry_files.extend(other.retry_files);
        self.total_size_uploaded += other.total_size_uploaded;
    }
}

/// Information about a file to download.
#[derive(Debug, Clone)]
struct DownloadInfo {
    item_path: String,
    local_path: PathBuf,
}

/// Holds the results of a parallel download operation.
#[derive(Debug, Default)]
struct DownloadResult {
    failed_files: Vec<DownloadInfo>,
}

impl DownloadResult {
    #[allow(dead_code)]
    fn merge(&mut self, other: DownloadResult) {
        self.failed_files.extend(other.failed_files);
    }
}

impl FileContainerServer {
    /// Create a new `FileContainerServer`.
    ///
    /// * `client`         – a pre-configured `reqwest::Client`
    /// * `base_url`       – base URL of the file container service
    /// * `auth_token`     – OAuth access token
    /// * `project_id`     – project GUID (usually `Guid::Empty` for Actions)
    /// * `container_id`   – the numeric container ID
    /// * `container_path` – the virtual path prefix inside the container
    pub fn new(
        client: Client,
        base_url: &str,
        auth_token: &str,
        project_id: Uuid,
        container_id: i64,
        container_path: &str,
    ) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token: auth_token.to_string(),
            project_id,
            container_id,
            container_path: container_path.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // REST URL helpers
    // -----------------------------------------------------------------------

    fn query_container_url(&self) -> String {
        format!(
            "{base}/_apis/resources/Containers/{cid}?itemPath={path}&isShallow=false&api-version=4.1-preview.4",
            base = self.base_url,
            cid = self.container_id,
            path = percent_encoding::utf8_percent_encode(
                &self.container_path,
                percent_encoding::NON_ALPHANUMERIC,
            ),
        )
    }

    fn upload_file_url(&self, item_path: &str) -> String {
        format!(
            "{base}/_apis/resources/Containers/{cid}?itemPath={path}&api-version=4.1-preview.4",
            base = self.base_url,
            cid = self.container_id,
            path = percent_encoding::utf8_percent_encode(
                item_path,
                percent_encoding::NON_ALPHANUMERIC,
            ),
        )
    }

    fn download_file_url(&self, item_path: &str) -> String {
        format!(
            "{base}/_apis/resources/Containers/{cid}?itemPath={path}&api-version=4.1-preview.4&$format=octetStream",
            base = self.base_url,
            cid = self.container_id,
            path = percent_encoding::utf8_percent_encode(
                item_path,
                percent_encoding::NON_ALPHANUMERIC,
            ),
        )
    }

    // -----------------------------------------------------------------------
    // Public API – Download
    // -----------------------------------------------------------------------

    /// Download all files in the container to `destination`.
    ///
    /// Mirrors `DownloadFromContainerAsync` from the C# implementation.
    pub async fn download_from_container(
        &self,
        trace: &dyn TraceWriter,
        destination: &str,
    ) -> Result<()> {
        // Query container items with retry
        let container_items = self.query_container_items_with_retry(trace).await?;

        if container_items.is_empty() {
            trace.info(&format!(
                "There is nothing under #/{}/{}",
                self.container_id, self.container_path
            ));
            return Ok(());
        }

        // Sort items by path for deterministic ordering
        let mut items = container_items;
        items.sort_by(|a, b| a.path.cmp(&b.path));

        let mut folders_created: u32 = 0;
        let mut empty_files_created: u32 = 0;
        let mut download_files: Vec<DownloadInfo> = Vec::new();

        for item in &items {
            // Verify the item path starts with the container path
            if !item
                .path
                .to_lowercase()
                .starts_with(&self.container_path.to_lowercase())
            {
                anyhow::bail!(
                    "Item {} is not under #/{}/{}",
                    item.path,
                    self.container_id,
                    self.container_path,
                );
            }

            let local_relative_path = item.path[self.container_path.len()..]
                .trim_start_matches('/');
            let local_path = Path::new(destination).join(local_relative_path);

            match item.item_type {
                ContainerItemType::Folder => {
                    trace.verbose(&format!("Ensure folder exists: {}", local_path.display()));
                    fs::create_dir_all(&local_path).await.with_context(|| {
                        format!("Failed to create directory {}", local_path.display())
                    })?;
                    folders_created += 1;
                }
                ContainerItemType::File => {
                    if item.file_length == 0 {
                        trace.verbose(&format!(
                            "Create empty file at: {}",
                            local_path.display()
                        ));
                        if let Some(parent) = local_path.parent() {
                            fs::create_dir_all(parent).await?;
                        }
                        // Create (or truncate) the file
                        fs::write(&local_path, b"").await.with_context(|| {
                            format!("Failed to create empty file {}", local_path.display())
                        })?;
                        empty_files_created += 1;
                    } else {
                        trace.verbose(&format!(
                            "Prepare download {} to {}",
                            item.path,
                            local_path.display()
                        ));
                        download_files.push(DownloadInfo {
                            item_path: item.path.clone(),
                            local_path,
                        });
                    }
                }
            }
        }

        if folders_created > 0 {
            trace.info(&format!("{folders_created} folders created."));
        }
        if empty_files_created > 0 {
            trace.info(&format!("{empty_files_created} empty files created."));
        }
        if download_files.is_empty() {
            trace.info("There is nothing to download");
            return Ok(());
        }

        // First attempt – parallel download
        let concurrency = std::cmp::min(download_files.len(), num_cpus());
        let mut result = self
            .parallel_download(trace, &download_files, concurrency)
            .await;

        if result.failed_files.is_empty() {
            trace.info(&format!(
                "{} files download succeed.",
                download_files.len()
            ));
            return Ok(());
        }

        trace.info(&format!(
            "{} files failed to download, retry these files after a minute.",
            result.failed_files.len()
        ));

        // Wait ~60 seconds then retry
        let mut timer = 60i32;
        while timer > 0 {
            trace.info(&format!("Retry file download after {timer} seconds."));
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            timer -= 5;
        }

        let failed_for_retry: Vec<DownloadInfo> = std::mem::take(&mut result.failed_files);
        let retry_count = failed_for_retry.len();
        trace.info(&format!("Start retry {retry_count} failed files download."));

        let retry_result = self
            .parallel_download(trace, &failed_for_retry, std::cmp::min(retry_count, num_cpus()))
            .await;

        if retry_result.failed_files.is_empty() {
            trace.info(&format!(
                "{retry_count} files download succeed after retry."
            ));
            Ok(())
        } else {
            anyhow::bail!(
                "{} files failed to download even after retry.",
                retry_result.failed_files.len()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Public API – Upload
    // -----------------------------------------------------------------------

    /// Upload all files from `source` (file or directory) into the container.
    ///
    /// Returns the total number of bytes uploaded.
    /// Mirrors `CopyToContainerAsync` from the C# implementation.
    pub async fn copy_to_container(
        &self,
        trace: &dyn TraceWriter,
        source: &str,
    ) -> Result<i64> {
        let source_path = Path::new(source);
        let (files, source_parent_directory) = if source_path.is_file() {
            let parent = source_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            (vec![source_path.to_path_buf()], parent)
        } else {
            let mut collected = Vec::new();
            collect_files_recursive(source_path, &mut collected).await?;
            let parent = PathBuf::from(
                source
                    .trim_end_matches(std::path::MAIN_SEPARATOR)
                    .trim_end_matches('/'),
            );
            (collected, parent)
        };

        let max_concurrent = std::cmp::min(num_cpus(), MAX_CONCURRENT_UPLOADS);
        trace.info(&format!("Uploading {} files", files.len()));

        // First attempt
        let mut upload_result = self
            .parallel_upload(trace, &files, &source_parent_directory, max_concurrent)
            .await;

        if upload_result.retry_files.is_empty() {
            trace.info("File upload complete.");
            return Ok(upload_result.total_size_uploaded);
        }

        trace.info(&format!(
            "{} files failed to upload, retry these files after a minute.",
            upload_result.retry_files.len()
        ));

        // Wait ~60 seconds then retry
        let mut timer = 60i32;
        while timer > 0 {
            trace.info(&format!("Retry file upload after {timer} seconds."));
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            timer -= 5;
        }

        let retry_list: Vec<String> = std::mem::take(&mut upload_result.retry_files);
        let retry_paths: Vec<PathBuf> = retry_list.into_iter().map(PathBuf::from).collect();
        let retry_count = retry_paths.len();
        trace.info(&format!(
            "Start retry {retry_count} failed files upload."
        ));

        let retry_result = self
            .parallel_upload(
                trace,
                &retry_paths,
                &source_parent_directory,
                max_concurrent,
            )
            .await;

        if retry_result.retry_files.is_empty() {
            trace.info("File upload complete after retry.");
            Ok(upload_result.total_size_uploaded + retry_result.total_size_uploaded)
        } else {
            anyhow::bail!("File upload failed even after retry.");
        }
    }

    // -----------------------------------------------------------------------
    // Private – query container items
    // -----------------------------------------------------------------------

    async fn query_container_items_with_retry(
        &self,
        trace: &dyn TraceWriter,
    ) -> Result<Vec<FileContainerItem>> {
        let mut retries = 0u32;
        loop {
            match self.query_container_items().await {
                Ok(items) => return Ok(items),
                Err(e) if retries < 2 => {
                    retries += 1;
                    trace.warning(&format!(
                        "Fail to query container items under #/{}/{}, Error: {}",
                        self.container_id, self.container_path, e
                    ));
                    let backoff = random_backoff_secs(5, 15);
                    trace.warning(&format!(
                        "Back off {backoff} seconds before retry."
                    ));
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn query_container_items(&self) -> Result<Vec<FileContainerItem>> {
        let url = self.query_container_url();

        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .context("Failed to query container items")?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to query container items (HTTP {status}): {text}");
        }

        // The API returns either `{ "value": [...] }` or a raw array.
        let text = response.text().await?;
        // Try the wrapper format first.
        if let Ok(wrapper) = serde_json::from_str::<ContainerItemsWrapper>(&text) {
            return Ok(wrapper.value);
        }
        // Fall back to a raw array.
        let items: Vec<FileContainerItem> = serde_json::from_str(&text)
            .context("Failed to deserialize container items")?;
        Ok(items)
    }

    // -----------------------------------------------------------------------
    // Private – parallel download
    // -----------------------------------------------------------------------

    async fn parallel_download(
        &self,
        _trace: &dyn TraceWriter,
        files: &[DownloadInfo],
        concurrency: usize,
    ) -> DownloadResult {
        if files.is_empty() {
            return DownloadResult::default();
        }

        let semaphore = Arc::new(Semaphore::new(concurrency));
        let files_processed = Arc::new(AtomicI32::new(0));
        let total = files.len();

        // We'll collect results via a channel.
        let mut handles = Vec::with_capacity(total);

        for file_info in files.iter() {
            let sem = semaphore.clone();
            let client = self.client.clone();
            let auth = self.auth_token.clone();
            let url = self.download_file_url(&file_info.item_path);
            let item_path = file_info.item_path.clone();
            let local_path = file_info.local_path.clone();
            let processed = files_processed.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let result = download_single_file(&client, &auth, &url, &item_path, &local_path).await;
                processed.fetch_add(1, Ordering::Relaxed);
                match result {
                    Ok(()) => None,
                    Err(_e) => Some(DownloadInfo {
                        item_path,
                        local_path,
                    }),
                }
            });
            handles.push(handle);
        }

        let mut result = DownloadResult::default();
        for handle in handles {
            if let Ok(Some(failed)) = handle.await {
                result.failed_files.push(failed);
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Private – parallel upload
    // -----------------------------------------------------------------------

    async fn parallel_upload(
        &self,
        _trace: &dyn TraceWriter,
        files: &[PathBuf],
        source_parent_directory: &Path,
        concurrency: usize,
    ) -> UploadResult {
        if files.is_empty() {
            return UploadResult::default();
        }

        let semaphore = Arc::new(Semaphore::new(concurrency));
        let files_processed = Arc::new(AtomicI32::new(0));
        let total = files.len();

        let mut handles = Vec::with_capacity(total);

        for file_path in files {
            let sem = semaphore.clone();
            let client = self.client.clone();
            let auth = self.auth_token.clone();

            // Build the container item path:
            //   container_path.trim_end('/') + "/" + relative_path_from_source
            let relative = file_path
                .strip_prefix(source_parent_directory)
                .unwrap_or(file_path);
            let item_path = format!(
                "{}/{}",
                self.container_path.trim_end_matches('/'),
                relative.to_string_lossy().replace('\\', "/"),
            );
            let upload_url = self.upload_file_url(&item_path);
            let file_path_owned = file_path.clone();
            let processed = files_processed.clone();

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let result = upload_single_file(&client, &auth, &upload_url, &file_path_owned).await;
                processed.fetch_add(1, Ordering::Relaxed);
                match result {
                    Ok(size) => (None, size),
                    Err(_e) => (Some(file_path_owned.to_string_lossy().to_string()), 0i64),
                }
            });
            handles.push(handle);
        }

        let mut result = UploadResult::default();
        for handle in handles {
            if let Ok((failed, size)) = handle.await {
                result.total_size_uploaded += size;
                if let Some(path) = failed {
                    result.retry_files.push(path);
                }
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Free-standing async helpers
// ---------------------------------------------------------------------------

/// Download a single file from the file container with retry.
async fn download_single_file(
    client: &Client,
    auth: &str,
    url: &str,
    item_path: &str,
    local_path: &Path,
) -> Result<()> {
    let mut retry_count = 0u32;
    loop {
        match attempt_download(client, auth, url, local_path).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                retry_count += 1;
                if retry_count >= MAX_RETRIES {
                    return Err(e).with_context(|| {
                        format!("Download of '{}' failed after {} retries", item_path, MAX_RETRIES)
                    });
                }
                let backoff = random_backoff_secs(10, 30);
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            }
        }
    }
}

async fn attempt_download(client: &Client, auth: &str, url: &str, local_path: &Path) -> Result<()> {
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let response = client
        .get(url)
        .bearer_auth(auth)
        .send()
        .await
        .context("Failed to send download request")?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Download failed (HTTP {status})");
    }

    let bytes = response.bytes().await.context("Failed to read download body")?;
    let mut file = fs::File::create(local_path).await.with_context(|| {
        format!("Failed to create file {}", local_path.display())
    })?;
    file.write_all(&bytes).await?;
    file.flush().await?;

    Ok(())
}

/// Upload a single file to the file container with retry.
///
/// Returns the number of bytes uploaded on success.
async fn upload_single_file(
    client: &Client,
    auth: &str,
    url: &str,
    file_path: &Path,
) -> Result<i64> {
    let mut retry_count = 0u32;
    loop {
        match attempt_upload(client, auth, url, file_path).await {
            Ok(size) => return Ok(size),
            Err(e) => {
                retry_count += 1;
                if retry_count >= MAX_RETRIES {
                    return Err(e).with_context(|| {
                        format!(
                            "Upload of '{}' failed after {} retries",
                            file_path.display(),
                            MAX_RETRIES,
                        )
                    });
                }
                let backoff = random_backoff_secs(5, 15);
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            }
        }
    }
}

async fn attempt_upload(client: &Client, auth: &str, url: &str, file_path: &Path) -> Result<i64> {
    let data = fs::read(file_path).await.with_context(|| {
        format!("Failed to read file for upload: {}", file_path.display())
    })?;
    let file_size = data.len() as i64;

    let response = client
        .put(url)
        .bearer_auth(auth)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", file_size.to_string())
        .header(
            "Content-Range",
            format!("bytes 0-{}/{}", file_size.saturating_sub(1).max(0), file_size),
        )
        .body(data)
        .send()
        .await
        .context("Failed to send upload request")?;

    let status = response.status();
    if status == StatusCode::CONFLICT {
        anyhow::bail!("File '{}' has already been uploaded.", file_path.display());
    }
    if status != StatusCode::CREATED && !status.is_success() {
        let reason = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "Unable to copy file to server StatusCode={status}: {reason}. Source file path: {}",
            file_path.display(),
        );
    }

    Ok(file_size)
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Recursively collect all files under `dir`.
async fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut read_dir = fs::read_dir(dir).await.with_context(|| {
        format!("Failed to read directory {}", dir.display())
    })?;

    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            Box::pin(collect_files_recursive(&path, out)).await?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Simple random backoff in the range `[min_secs, max_secs]`.
fn random_backoff_secs(min_secs: u64, max_secs: u64) -> u64 {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    rng.gen_range(min_secs..=max_secs)
}

/// Get the number of logical CPUs (clamped to at least 1).
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Wrapper type for the `{ "value": [...] }` envelope the REST API may return.
#[derive(Debug, Deserialize)]
struct ContainerItemsWrapper {
    value: Vec<FileContainerItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_result_merge() {
        let mut a = UploadResult {
            retry_files: vec!["a.txt".into()],
            total_size_uploaded: 100,
        };
        let b = UploadResult {
            retry_files: vec!["b.txt".into()],
            total_size_uploaded: 200,
        };
        a.merge(b);
        assert_eq!(a.retry_files.len(), 2);
        assert_eq!(a.total_size_uploaded, 300);
    }

    #[test]
    fn download_result_merge() {
        let mut a = DownloadResult {
            failed_files: vec![DownloadInfo {
                item_path: "a".into(),
                local_path: PathBuf::from("/tmp/a"),
            }],
        };
        let b = DownloadResult {
            failed_files: vec![DownloadInfo {
                item_path: "b".into(),
                local_path: PathBuf::from("/tmp/b"),
            }],
        };
        a.merge(b);
        assert_eq!(a.failed_files.len(), 2);
    }

    #[test]
    fn query_container_url_format() {
        let server = FileContainerServer::new(
            Client::new(),
            "https://example.com",
            "tok",
            Uuid::nil(),
            42,
            "my/path",
        );
        let url = server.query_container_url();
        assert!(url.contains("/Containers/42"));
        assert!(url.contains("itemPath="));
        assert!(url.contains("api-version=4.1-preview.4"));
    }

    #[test]
    fn random_backoff_in_range() {
        for _ in 0..100 {
            let v = random_backoff_secs(5, 15);
            assert!(v >= 5 && v <= 15);
        }
    }

    #[test]
    fn container_item_deserialization() {
        let json = r#"{"path":"artifacts/file.txt","itemType":"file","fileLength":1024}"#;
        let item: FileContainerItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.path, "artifacts/file.txt");
        assert_eq!(item.item_type, ContainerItemType::File);
        assert_eq!(item.file_length, 1024);
    }

    #[test]
    fn container_item_folder() {
        let json = r#"{"path":"artifacts/subdir","itemType":"folder","fileLength":0}"#;
        let item: FileContainerItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.item_type, ContainerItemType::Folder);
    }

    #[tokio::test]
    async fn collect_files_recursive_works() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("sub");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        tokio::fs::write(tmp.path().join("a.txt"), b"a").await.unwrap();
        tokio::fs::write(sub.join("b.txt"), b"b").await.unwrap();
        let mut files = Vec::new();
        collect_files_recursive(tmp.path(), &mut files).await.unwrap();
        assert_eq!(files.len(), 2);
    }
}
