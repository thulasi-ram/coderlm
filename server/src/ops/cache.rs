use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::index::file_entry::FileEntry;
use crate::index::file_tree::FileTree;
use crate::index::walker;
use crate::symbols::symbol::Symbol;
use crate::symbols::SymbolTable;

const INDEX_FILE: &str = ".coderlm/index.bin";
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct IndexCache {
    version: u32,
    file_entries: HashMap<String, FileEntry>,
    symbols: HashMap<String, Symbol>,
    symbols_by_name: HashMap<String, HashSet<String>>,
    symbols_by_file: HashMap<String, HashSet<String>>,
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

/// Save the current file tree and symbol table to `.coderlm/index.bin`.
pub fn save_index(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
) -> Result<(), String> {
    let file_entries: HashMap<String, FileEntry> = file_tree
        .files
        .iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect();

    let symbols: HashMap<String, Symbol> = symbol_table
        .symbols
        .iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect();

    let symbols_by_name: HashMap<String, HashSet<String>> = symbol_table
        .by_name
        .iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect();

    let symbols_by_file: HashMap<String, HashSet<String>> = symbol_table
        .by_file
        .iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect();

    let cache = IndexCache {
        version: CACHE_VERSION,
        file_entries,
        symbols,
        symbols_by_name,
        symbols_by_file,
    };

    let index_path = root.join(INDEX_FILE);
    if let Some(parent) = index_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
    }

    let bytes = bincode::serialize(&cache)
        .map_err(|e| format!("Failed to serialize index cache: {}", e))?;
    std::fs::write(&index_path, bytes)
        .map_err(|e| format!("Failed to write index cache: {}", e))?;

    info!(
        "Saved index cache: {} files, {} symbols to {}",
        cache.file_entries.len(),
        cache.symbols.len(),
        index_path.display()
    );

    Ok(())
}

/// Load a cached index from `.coderlm/index.bin`, compare against the current
/// filesystem state, and populate the file tree and symbol table. Returns stats
/// and a list of files needing symbol re-extraction.
pub fn load_index(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
    max_file_size: u64,
) -> Result<CacheLoadStats, String> {
    let index_path = root.join(INDEX_FILE);
    if !index_path.exists() {
        return Err("No index cache found".into());
    }

    let bytes = std::fs::read(&index_path)
        .map_err(|e| format!("Failed to read index cache: {}", e))?;
    let cache: IndexCache = bincode::deserialize(&bytes)
        .map_err(|e| format!("Failed to deserialize index cache: {}", e))?;

    if cache.version != CACHE_VERSION {
        return Err(format!(
            "Cache version mismatch: got {}, expected {}",
            cache.version, CACHE_VERSION
        ));
    }

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

    // Build a set of cached file paths for deletion detection
    let cached_paths: HashSet<String> = cache.file_entries.keys().cloned().collect();
    let mut fresh_paths: HashSet<String> = HashSet::new();

    // Process each file from the fresh scan
    for entry in fresh_tree.files.iter() {
        let rel_path = entry.key().clone();
        let fresh_entry = entry.value().clone();
        fresh_paths.insert(rel_path.clone());

        if let Some(cached_entry) = cache.file_entries.get(&rel_path) {
            if cached_entry.modified == fresh_entry.modified
                && cached_entry.size == fresh_entry.size
            {
                // Unchanged — use cached entry (preserves symbols_extracted, annotations)
                file_tree.insert(cached_entry.clone());
                stats.cached += 1;
            } else {
                // Modified — use fresh entry, mark for re-extraction
                file_tree.insert(fresh_entry);
                stats.changed += 1;
                stats.files_to_extract.push(rel_path.clone());
                // Remove stale symbols for this file
                debug!("File changed: {}", rel_path);
            }
        } else {
            // New file — insert fresh, mark for extraction
            file_tree.insert(fresh_entry);
            stats.new += 1;
            stats.files_to_extract.push(rel_path.clone());
            debug!("New file: {}", rel_path);
        }
    }

    // Count deleted files (in cache but not on disk)
    for path in &cached_paths {
        if !fresh_paths.contains(path) {
            stats.deleted += 1;
            debug!("Deleted file: {}", path);
        }
    }

    // Populate symbol table with cached symbols, skipping those from changed/deleted files
    let stale_files: HashSet<&String> = stats
        .files_to_extract
        .iter()
        .chain(cached_paths.difference(&fresh_paths))
        .collect();

    for (key, symbol) in &cache.symbols {
        if !stale_files.contains(&symbol.file) {
            symbol_table.symbols.insert(key.clone(), symbol.clone());
        }
    }

    // Rebuild secondary indices from the symbols we kept
    for (name, keys) in &cache.symbols_by_name {
        let valid_keys: HashSet<String> = keys
            .iter()
            .filter(|k| symbol_table.symbols.contains_key(k.as_str()))
            .cloned()
            .collect();
        if !valid_keys.is_empty() {
            symbol_table.by_name.insert(name.clone(), valid_keys);
        }
    }

    for (file, keys) in &cache.symbols_by_file {
        if stale_files.contains(file) {
            continue;
        }
        let valid_keys: HashSet<String> = keys
            .iter()
            .filter(|k| symbol_table.symbols.contains_key(k.as_str()))
            .cloned()
            .collect();
        if !valid_keys.is_empty() {
            symbol_table.by_file.insert(file.clone(), valid_keys);
        }
    }

    info!(
        "Loaded index cache: {} cached, {} changed, {} new, {} deleted, {} to re-extract",
        stats.cached,
        stats.changed,
        stats.new,
        stats.deleted,
        stats.files_to_extract.len()
    );

    Ok(stats)
}
