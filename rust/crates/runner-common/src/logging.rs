// PagingLogger mapping `Logging.cs`.
// Writes log output to paged files with UTC timestamps.

use anyhow::Result;
use chrono::Utc;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use uuid::Uuid;

/// Folder name for log pages.
pub const PAGING_FOLDER: &str = "pages";

/// Maximum size of a single log page in bytes (8 MB).
pub const PAGE_SIZE: usize = 8 * 1024 * 1024;

/// Folder name for result blocks.
pub const BLOCKS_FOLDER: &str = "blocks";

/// Maximum size of a single result block in bytes (2 MB).
pub const BLOCK_SIZE: usize = 2 * 1024 * 1024;

/// A logger that writes output to paged log files on disk.
///
/// Each page is capped at `PAGE_SIZE` bytes. When a page fills up,
/// a new page file is created. Each line is prepended with a UTC timestamp.
///
/// Maps `PagingLogger` in the C# runner.
pub struct PagingLogger {
    timeline_id: Uuid,
    timeline_record_id: Uuid,

    /// Pages directory
    pages_folder: PathBuf,
    /// Current page writer
    page_writer: Option<BufWriter<File>>,
    /// Current page file path
    page_data_file: Option<PathBuf>,
    /// Byte count in current page
    page_byte_count: usize,
    /// Page counter
    page_count: u32,

    /// Blocks directory for results
    blocks_folder: PathBuf,
    /// Current block writer
    block_writer: Option<BufWriter<File>>,
    /// Current block file path
    block_data_file: Option<PathBuf>,
    /// Byte count in current block
    block_byte_count: usize,
    /// Block counter
    block_count: u32,

    /// Total lines written
    total_lines: u64,

    /// Callback invoked when a page is complete (for upload queueing).
    on_page_complete: Option<Box<dyn Fn(Uuid, Uuid, &str) + Send + Sync>>,
    /// Callback invoked when a block is complete.
    on_block_complete: Option<Box<dyn Fn(Uuid, &str, bool, bool, u64) + Send + Sync>>,
}

impl PagingLogger {
    /// Create a new `PagingLogger` writing to the given diag directory.
    pub fn new(diag_directory: &std::path::Path) -> Result<Self> {
        let pages_folder = diag_directory.join(PAGING_FOLDER);
        fs::create_dir_all(&pages_folder)?;

        let blocks_folder = diag_directory.join(BLOCKS_FOLDER);
        fs::create_dir_all(&blocks_folder)?;

        Ok(Self {
            timeline_id: Uuid::nil(),
            timeline_record_id: Uuid::nil(),
            pages_folder,
            page_writer: None,
            page_data_file: None,
            page_byte_count: 0,
            page_count: 0,
            blocks_folder,
            block_writer: None,
            block_data_file: None,
            block_byte_count: 0,
            block_count: 0,
            total_lines: 0,
            on_page_complete: None,
            on_block_complete: None,
        })
    }

    /// Set up the logger for a specific timeline record.
    pub fn setup(&mut self, timeline_id: Uuid, timeline_record_id: Uuid) {
        self.timeline_id = timeline_id;
        self.timeline_record_id = timeline_record_id;
    }

    /// Set the callback invoked when a page file is complete.
    pub fn set_on_page_complete<F>(&mut self, callback: F)
    where
        F: Fn(Uuid, Uuid, &str) + Send + Sync + 'static,
    {
        self.on_page_complete = Some(Box::new(callback));
    }

    /// Set the callback invoked when a result block is complete.
    pub fn set_on_block_complete<F>(&mut self, callback: F)
    where
        F: Fn(Uuid, &str, bool, bool, u64) + Send + Sync + 'static,
    {
        self.on_block_complete = Some(Box::new(callback));
    }

    /// Get the total number of lines written.
    pub fn total_lines(&self) -> u64 {
        self.total_lines
    }

    /// Write a message to the log. The message is prepended with a UTC timestamp.
    pub fn write(&mut self, message: &str) {
        // Lazy creation on first write
        if self.page_writer.is_none() {
            self.new_page();
        }
        if self.block_writer.is_none() {
            self.new_block();
        }

        let line = format!("{} {}", Utc::now().format("%Y-%m-%dT%H:%M:%S%.7fZ"), message);

        // Write to page
        if let Some(ref mut writer) = self.page_writer {
            let _ = writeln!(writer, "{}", line);
        }

        // Write to block
        if let Some(ref mut writer) = self.block_writer {
            let _ = writeln!(writer, "{}", line);
        }

        // Count lines (including embedded newlines)
        self.total_lines += 1;
        self.total_lines += line.chars().filter(|&c| c == '\n').count() as u64;

        let byte_len = line.len() + 1; // +1 for the newline
        self.page_byte_count += byte_len;
        self.block_byte_count += byte_len;

        if self.page_byte_count >= PAGE_SIZE {
            self.new_page();
        }

        if self.block_byte_count >= BLOCK_SIZE {
            self.new_block();
        }
    }

    /// Finalize the logger, flushing and closing all open files.
    pub fn end(&mut self) {
        self.end_page();
        self.end_block(true);
    }

    /// Start a new page file.
    fn new_page(&mut self) {
        self.end_page();
        self.page_byte_count = 0;
        self.page_count += 1;

        let file_name = format!(
            "{}_{}_{}.log",
            self.timeline_id, self.timeline_record_id, self.page_count
        );
        let path = self.pages_folder.join(&file_name);

        match File::create(&path) {
            Ok(file) => {
                self.page_writer = Some(BufWriter::new(file));
                self.page_data_file = Some(path);
            }
            Err(e) => {
                tracing::error!("Failed to create page file {:?}: {}", path, e);
            }
        }
    }

    /// Close and finalize the current page.
    fn end_page(&mut self) {
        if let Some(mut writer) = self.page_writer.take() {
            let _ = writer.flush();
        }
        if let Some(ref path) = self.page_data_file.take() {
            if let Some(ref callback) = self.on_page_complete {
                callback(
                    self.timeline_id,
                    self.timeline_record_id,
                    path.to_str().unwrap_or(""),
                );
            }
        }
    }

    /// Start a new result block.
    fn new_block(&mut self) {
        self.end_block(false);
        self.block_byte_count = 0;
        self.block_count += 1;

        let file_name = format!(
            "{}_{}.{}",
            self.timeline_id, self.timeline_record_id, self.block_count
        );
        let path = self.blocks_folder.join(&file_name);

        match File::create(&path) {
            Ok(file) => {
                self.block_writer = Some(BufWriter::new(file));
                self.block_data_file = Some(path);
            }
            Err(e) => {
                tracing::error!("Failed to create block file {:?}: {}", path, e);
            }
        }
    }

    /// Close and finalize the current result block.
    fn end_block(&mut self, finalize: bool) {
        if let Some(mut writer) = self.block_writer.take() {
            let _ = writer.flush();
        }
        if let Some(ref path) = self.block_data_file.take() {
            let first_block = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".1"))
                .unwrap_or(false);

            if let Some(ref callback) = self.on_block_complete {
                callback(
                    self.timeline_record_id,
                    path.to_str().unwrap_or(""),
                    finalize,
                    first_block,
                    self.total_lines,
                );
            }
        }
    }
}

impl Drop for PagingLogger {
    fn drop(&mut self) {
        self.end();
    }
}
