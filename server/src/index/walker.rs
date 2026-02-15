use anyhow::Result;
use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::info;

use crate::config;
use crate::index::file_entry::FileEntry;
use crate::index::file_tree::FileTree;

/// Scan the codebase directory using the `ignore` crate (respects .gitignore)
/// plus our built-in ignore patterns. Uses parallel walking for performance.
/// Returns the number of files indexed.
pub fn scan_directory(root: &Path, file_tree: &Arc<FileTree>, max_file_size: u64) -> Result<usize> {
    let count = Arc::new(AtomicUsize::new(0));
    let root_buf = root.to_path_buf();

    WalkBuilder::new(root)
        .hidden(true) // skip dotfiles by default
        .git_ignore(true) // respect .gitignore
        .git_global(true)
        .git_exclude(true)
        .threads(num_threads())
        .build_parallel()
        .run(|| {
            let file_tree = file_tree.clone();
            let count = count.clone();
            let root = root_buf.clone();

            Box::new(move |entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => return ignore::WalkState::Continue,
                };

                // Skip directories
                if entry.file_type().map_or(true, |ft| ft.is_dir()) {
                    return ignore::WalkState::Continue;
                }

                let path = entry.path();

                // Get the relative path
                let rel_path = match path.strip_prefix(&root) {
                    Ok(r) => r.to_string_lossy().to_string(),
                    Err(_) => return ignore::WalkState::Continue,
                };

                // Apply our additional ignore rules
                if should_skip(&rel_path) {
                    return ignore::WalkState::Continue;
                }

                // Check extension-based ignoring
                if config::should_ignore_extension(&rel_path) {
                    return ignore::WalkState::Continue;
                }

                // Get file metadata
                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => return ignore::WalkState::Continue,
                };

                let size = metadata.len();

                // Skip files over size limit
                if size > max_file_size {
                    return ignore::WalkState::Continue;
                }

                let modified: DateTime<Utc> = metadata
                    .modified()
                    .map(DateTime::from)
                    .unwrap_or_else(|_| Utc::now());

                let file_entry = FileEntry::new(rel_path, size, modified);
                file_tree.insert(file_entry);
                count.fetch_add(1, Ordering::Relaxed);

                ignore::WalkState::Continue
            })
        });

    let total = count.load(Ordering::Relaxed);
    info!("Scanned {} files from {}", total, root.display());
    Ok(total)
}

/// Check if any path component matches our built-in ignore directories.
fn should_skip(rel_path: &str) -> bool {
    for component in rel_path.split('/') {
        if config::should_ignore_dir(component) {
            return true;
        }
    }
    false
}

/// Number of threads for parallel walking. Uses available parallelism, capped reasonably.
fn num_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}
