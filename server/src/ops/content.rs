use std::path::Path;
use std::sync::Arc;

use regex::Regex;
use serde::Serialize;

use crate::index::file_tree::FileTree;

#[derive(Debug, Serialize)]
pub struct PeekResponse {
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub content: String,
}

pub fn peek(
    root: &Path,
    file_tree: &Arc<FileTree>,
    file: &str,
    start: usize,
    end: usize,
) -> Result<PeekResponse, String> {
    if file_tree.get(file).is_none() {
        return Err(format!("File '{}' not found in index", file));
    }

    let abs_path = root.join(file);
    let source =
        std::fs::read_to_string(&abs_path).map_err(|e| format!("Failed to read '{}': {}", file, e))?;

    let lines: Vec<&str> = source.lines().collect();
    let total_lines = lines.len();
    let start = start.min(total_lines);
    let end = end.min(total_lines);

    let content: String = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6} │ {}", start + i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(PeekResponse {
        file: file.to_string(),
        start_line: start + 1,
        end_line: end,
        total_lines,
        content,
    })
}

#[derive(Debug, Serialize)]
pub struct GrepResponse {
    pub pattern: String,
    pub matches: Vec<GrepMatch>,
    pub total_matches: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct GrepMatch {
    pub file: String,
    pub line: usize,
    pub text: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

pub fn grep(
    root: &Path,
    file_tree: &Arc<FileTree>,
    pattern: &str,
    max_matches: usize,
    context_lines: usize,
) -> Result<GrepResponse, String> {
    let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {}", e))?;

    let mut matches = Vec::new();
    let mut total = 0;

    let mut paths: Vec<String> = file_tree.all_paths();
    paths.sort();

    for rel_path in &paths {
        let abs_path = root.join(rel_path);
        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                total += 1;
                if matches.len() < max_matches {
                    let ctx_start = i.saturating_sub(context_lines);
                    let ctx_end = (i + context_lines + 1).min(lines.len());

                    let context_before: Vec<String> = lines[ctx_start..i]
                        .iter()
                        .map(|l| l.to_string())
                        .collect();
                    let context_after: Vec<String> = lines[(i + 1)..ctx_end]
                        .iter()
                        .map(|l| l.to_string())
                        .collect();

                    matches.push(GrepMatch {
                        file: rel_path.clone(),
                        line: i + 1,
                        text: line.to_string(),
                        context_before,
                        context_after,
                    });
                }
            }
        }
    }

    Ok(GrepResponse {
        pattern: pattern.to_string(),
        matches,
        total_matches: total,
        truncated: total > max_matches,
    })
}

#[derive(Debug, Serialize)]
pub struct ChunkIndicesResponse {
    pub file: String,
    pub total_bytes: usize,
    pub chunk_size: usize,
    pub overlap: usize,
    pub chunks: Vec<ChunkInfo>,
}

#[derive(Debug, Serialize)]
pub struct ChunkInfo {
    pub index: usize,
    pub start: usize,
    pub end: usize,
}

pub fn chunk_indices(
    root: &Path,
    file_tree: &Arc<FileTree>,
    file: &str,
    size: usize,
    overlap: usize,
) -> Result<ChunkIndicesResponse, String> {
    if size == 0 {
        return Err("Chunk size must be > 0".to_string());
    }
    if overlap >= size {
        return Err("Overlap must be < chunk size".to_string());
    }
    if file_tree.get(file).is_none() {
        return Err(format!("File '{}' not found in index", file));
    }

    let abs_path = root.join(file);
    let source =
        std::fs::read_to_string(&abs_path).map_err(|e| format!("Failed to read '{}': {}", file, e))?;

    let total_bytes = source.len();
    let step = size - overlap;
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut index = 0;

    while start < total_bytes {
        let end = (start + size).min(total_bytes);
        chunks.push(ChunkInfo { index, start, end });
        index += 1;
        start += step;
        if end >= total_bytes {
            break;
        }
    }

    Ok(ChunkIndicesResponse {
        file: file.to_string(),
        total_bytes,
        chunk_size: size,
        overlap,
        chunks,
    })
}
