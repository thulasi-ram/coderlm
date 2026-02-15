use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::index::file_entry::FileEntry;
use crate::index::file_tree::FileTree;
use crate::index::walker;
use crate::symbols::symbol::Symbol;
use crate::symbols::SymbolTable;

const CACHE_DIR: &str = ".coderlm/cache";
const MANIFEST_FILE: &str = ".coderlm/manifest.bin";
const CACHE_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: HashMap<String, ManifestEntry>,
}

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    size: u64,
    modified: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct FileCacheEntry {
    file_entry: FileEntry,
    symbols: Vec<Symbol>,
}

/// Stats returned after loading a cached index.
pub struct CacheLoadStats {
    pub cached: usize,
    pub changed: usize,
    pub new: usize,
    pub deleted: usize,
    /// Files that need symbol re-extraction (changed + new).
    pub files_to_extract: Vec<String>,
}

// ---------------------------------------------------------------------------
// Per-file cache helpers (public — used by parser.rs)
// ---------------------------------------------------------------------------

/// Cache path for a single file: `.coderlm/cache/<rel_path>.bin`
fn file_cache_path(root: &Path, rel_path: &str) -> std::path::PathBuf {
    root.join(CACHE_DIR).join(format!("{}.bin", rel_path))
}

/// Save a single file's cache entry to disk.
pub fn save_file_cache(
    root: &Path,
    file_entry: &FileEntry,
    symbols: &[Symbol],
) -> Result<(), String> {
    let entry = FileCacheEntry {
        file_entry: file_entry.clone(),
        symbols: symbols.to_vec(),
    };
    let path = file_cache_path(root, &file_entry.rel_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
    }
    let bytes = bincode::serialize(&entry)
        .map_err(|e| format!("Failed to serialize file cache: {}", e))?;
    std::fs::write(&path, bytes)
        .map_err(|e| format!("Failed to write file cache: {}", e))?;
    Ok(())
}

/// Load a single file's cache entry from disk.
fn load_file_cache(root: &Path, rel_path: &str) -> Result<FileCacheEntry, String> {
    let path = file_cache_path(root, rel_path);
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    bincode::deserialize(&bytes)
        .map_err(|e| format!("Failed to deserialize {}: {}", path.display(), e))
}

/// Delete a single file's cache entry from disk.
fn delete_file_cache(root: &Path, rel_path: &str) {
    let path = file_cache_path(root, rel_path);
    let _ = std::fs::remove_file(&path);
}

/// Save the manifest to disk.
fn save_manifest(root: &Path, manifest: &Manifest) -> Result<(), String> {
    let path = root.join(MANIFEST_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create manifest dir: {}", e))?;
    }
    let bytes = bincode::serialize(manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    std::fs::write(&path, bytes)
        .map_err(|e| format!("Failed to write manifest: {}", e))?;
    Ok(())
}

/// Load the manifest from disk.
fn load_manifest(root: &Path) -> Result<Manifest, String> {
    let path = root.join(MANIFEST_FILE);
    if !path.exists() {
        return Err("No manifest found".into());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("Failed to read manifest: {}", e))?;
    let manifest: Manifest = bincode::deserialize(&bytes)
        .map_err(|e| format!("Failed to deserialize manifest: {}", e))?;
    if manifest.version != CACHE_VERSION {
        return Err(format!(
            "Cache version mismatch: got {}, expected {}",
            manifest.version, CACHE_VERSION
        ));
    }
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Public API (same signatures as before)
// ---------------------------------------------------------------------------

/// Save the current file tree and symbol table as per-file cache entries.
pub fn save_index(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
) -> Result<(), String> {
    let cache_dir = root.join(CACHE_DIR);
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create cache dir: {}", e))?;

    let mut manifest = Manifest {
        version: CACHE_VERSION,
        files: HashMap::new(),
    };

    let mut saved = 0;
    for entry in file_tree.files.iter() {
        let rel_path = entry.key().clone();
        let file_entry = entry.value().clone();

        // Collect symbols for this file
        let symbols = symbol_table.list_by_file(&rel_path);

        if let Err(e) = save_file_cache(root, &file_entry, &symbols) {
            warn!("Failed to save cache for {}: {}", rel_path, e);
            continue;
        }

        manifest.files.insert(
            rel_path,
            ManifestEntry {
                size: file_entry.size,
                modified: file_entry.modified,
            },
        );
        saved += 1;
    }

    save_manifest(root, &manifest)?;

    info!(
        "Saved per-file cache: {} files, {} symbols",
        saved,
        symbol_table.len()
    );

    Ok(())
}

/// Load cached index from per-file cache, compare against current filesystem
/// state, and populate the file tree and symbol table. Returns stats and a
/// list of files needing symbol re-extraction.
pub fn load_index(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
    max_file_size: u64,
) -> Result<CacheLoadStats, String> {
    let manifest = load_manifest(root)?;

    // Do a fresh filesystem scan to get current state
    let fresh_tree = Arc::new(FileTree::new());
    walker::scan_directory(root, &fresh_tree, max_file_size)
        .map_err(|e| format!("Failed to scan directory: {}", e))?;

    let mut stats = CacheLoadStats {
        cached: 0,
        changed: 0,
        new: 0,
        deleted: 0,
        files_to_extract: Vec::new(),
    };

    let manifest_paths: HashSet<String> = manifest.files.keys().cloned().collect();
    let mut fresh_paths: HashSet<String> = HashSet::new();

    // Process each file from the fresh scan
    for entry in fresh_tree.files.iter() {
        let rel_path = entry.key().clone();
        let fresh_entry = entry.value().clone();
        fresh_paths.insert(rel_path.clone());

        if let Some(manifest_entry) = manifest.files.get(&rel_path) {
            if manifest_entry.modified == fresh_entry.modified
                && manifest_entry.size == fresh_entry.size
            {
                // Unchanged — load from per-file cache
                match load_file_cache(root, &rel_path) {
                    Ok(cached) => {
                        file_tree.insert(cached.file_entry);
                        for sym in cached.symbols {
                            symbol_table.insert(sym);
                        }
                        stats.cached += 1;
                    }
                    Err(e) => {
                        // Cache file missing/corrupt — treat as changed
                        debug!("Cache miss for {}: {}", rel_path, e);
                        file_tree.insert(fresh_entry);
                        stats.changed += 1;
                        stats.files_to_extract.push(rel_path.clone());
                    }
                }
            } else {
                // Modified — use fresh entry, mark for re-extraction
                file_tree.insert(fresh_entry);
                stats.changed += 1;
                stats.files_to_extract.push(rel_path.clone());
                debug!("File changed: {}", rel_path);
            }
        } else {
            // New file
            file_tree.insert(fresh_entry);
            stats.new += 1;
            stats.files_to_extract.push(rel_path.clone());
            debug!("New file: {}", rel_path);
        }
    }

    // Count deleted files
    for path in &manifest_paths {
        if !fresh_paths.contains(path) {
            stats.deleted += 1;
            delete_file_cache(root, path);
            debug!("Deleted file: {}", path);
        }
    }

    info!(
        "Loaded per-file cache: {} cached, {} changed, {} new, {} deleted, {} to re-extract",
        stats.cached,
        stats.changed,
        stats.new,
        stats.deleted,
        stats.files_to_extract.len()
    );

    Ok(stats)
}

/// Re-index a live project by diffing the in-memory FileTree against a fresh
/// filesystem scan. Updates the file tree in place and returns stats + a list
/// of files needing symbol re-extraction.
pub fn reindex(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
    max_file_size: u64,
) -> Result<CacheLoadStats, String> {
    // Fresh filesystem scan
    let fresh_tree = Arc::new(FileTree::new());
    walker::scan_directory(root, &fresh_tree, max_file_size)
        .map_err(|e| format!("Failed to scan directory: {}", e))?;

    let mut stats = CacheLoadStats {
        cached: 0,
        changed: 0,
        new: 0,
        deleted: 0,
        files_to_extract: Vec::new(),
    };

    // Collect current in-memory paths for deletion detection
    let current_paths: HashSet<String> = file_tree.all_paths().into_iter().collect();
    let mut fresh_paths: HashSet<String> = HashSet::new();

    // Process each file from the fresh scan
    for entry in fresh_tree.files.iter() {
        let rel_path = entry.key().clone();
        let fresh_entry = entry.value().clone();
        fresh_paths.insert(rel_path.clone());

        if let Some(existing) = file_tree.get(&rel_path) {
            if existing.modified == fresh_entry.modified
                && existing.size == fresh_entry.size
            {
                // Unchanged
                stats.cached += 1;
            } else {
                // Modified — update entry, remove stale symbols, mark for re-extraction
                file_tree.insert(fresh_entry);
                symbol_table.remove_file(&rel_path);
                stats.changed += 1;
                stats.files_to_extract.push(rel_path.clone());
                debug!("File changed: {}", rel_path);
            }
        } else {
            // New file
            file_tree.insert(fresh_entry);
            stats.new += 1;
            stats.files_to_extract.push(rel_path.clone());
            debug!("New file: {}", rel_path);
        }
    }

    // Remove deleted files
    for path in &current_paths {
        if !fresh_paths.contains(path) {
            file_tree.remove(path);
            symbol_table.remove_file(path);
            delete_file_cache(root, path);
            stats.deleted += 1;
            debug!("Deleted file: {}", path);
        }
    }

    // Save updated manifest reflecting current file tree state
    let mut manifest = Manifest {
        version: CACHE_VERSION,
        files: HashMap::new(),
    };
    for entry in file_tree.files.iter() {
        manifest.files.insert(
            entry.key().clone(),
            ManifestEntry {
                size: entry.value().size,
                modified: entry.value().modified,
            },
        );
    }
    if let Err(e) = save_manifest(root, &manifest) {
        warn!("Failed to save manifest after reindex: {}", e);
    }

    info!(
        "Reindex complete: {} unchanged, {} changed, {} new, {} deleted, {} to re-extract",
        stats.cached,
        stats.changed,
        stats.new,
        stats.deleted,
        stats.files_to_extract.len()
    );

    Ok(stats)
}
