use std::path::Path;
use std::sync::Arc;

use crate::symbols::symbol::{Symbol, SymbolKind};
use crate::symbols::SymbolTable;
use crate::index::file_tree::FileTree;

pub fn list_symbols(
    symbol_table: &Arc<SymbolTable>,
    kind_filter: Option<SymbolKind>,
    file_filter: Option<&str>,
    limit: usize,
) -> Vec<Symbol> {
    let mut results: Vec<Symbol> = if let Some(file) = file_filter {
        symbol_table.list_by_file(file)
    } else {
        symbol_table.all_symbols()
    };

    if let Some(kind) = kind_filter {
        results.retain(|s| s.kind == kind);
    }

    results.sort_by(|a, b| a.file.cmp(&b.file).then(a.line_range.0.cmp(&b.line_range.0)));
    results.truncate(limit);
    results
}

pub fn search_symbols(symbol_table: &Arc<SymbolTable>, query: &str, limit: usize) -> Vec<Symbol> {
    symbol_table.search(query, limit)
}

pub fn get_implementation(
    root: &Path,
    symbol_table: &Arc<SymbolTable>,
    symbol_name: &str,
    file: &str,
) -> Result<String, String> {
    let sym = symbol_table
        .get(file, symbol_name)
        .ok_or_else(|| format!("Symbol '{}' not found in '{}'", symbol_name, file))?;

    let abs_path = root.join(&sym.file);
    let source = std::fs::read_to_string(&abs_path)
        .map_err(|e| format!("Failed to read '{}': {}", sym.file, e))?;

    let start = sym.byte_range.0;
    let end = sym.byte_range.1.min(source.len());
    Ok(source[start..end].to_string())
}

pub fn define_symbol(
    symbol_table: &Arc<SymbolTable>,
    symbol_name: &str,
    file: &str,
    definition: &str,
) -> Result<(), String> {
    let key = SymbolTable::make_key(file, symbol_name);
    if let Some(mut sym) = symbol_table.symbols.get_mut(&key) {
        if sym.definition.is_some() {
            return Err(format!(
                "Symbol '{}' in '{}' already has a definition. Use redefine.",
                symbol_name, file
            ));
        }
        sym.definition = Some(definition.to_string());
        Ok(())
    } else {
        Err(format!("Symbol '{}' not found in '{}'", symbol_name, file))
    }
}

pub fn redefine_symbol(
    symbol_table: &Arc<SymbolTable>,
    symbol_name: &str,
    file: &str,
    definition: &str,
) -> Result<(), String> {
    let key = SymbolTable::make_key(file, symbol_name);
    if let Some(mut sym) = symbol_table.symbols.get_mut(&key) {
        sym.definition = Some(definition.to_string());
        Ok(())
    } else {
        Err(format!("Symbol '{}' not found in '{}'", symbol_name, file))
    }
}

/// Find callers of a symbol by grepping for its name and optionally
/// validating with tree-sitter that matches are call sites.
pub fn find_callers(
    root: &Path,
    file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
    symbol_name: &str,
    file: &str,
    limit: usize,
) -> Result<Vec<CallerInfo>, String> {
    // Verify symbol exists
    let _sym = symbol_table
        .get(file, symbol_name)
        .ok_or_else(|| format!("Symbol '{}' not found in '{}'", symbol_name, file))?;

    let pattern = regex::Regex::new(&regex::escape(symbol_name))
        .map_err(|e| format!("Invalid pattern: {}", e))?;

    let mut callers = Vec::new();

    for entry in file_tree.files.iter() {
        let rel_path = entry.key().clone();
        let abs_path = root.join(&rel_path);

        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for (line_num, line) in source.lines().enumerate() {
            if pattern.is_match(line) {
                // Skip the definition itself
                if rel_path == file && line.contains(&format!("fn {}", symbol_name)) {
                    continue;
                }
                if rel_path == file && line.contains(&format!("def {}", symbol_name)) {
                    continue;
                }
                if rel_path == file && line.contains(&format!("function {}", symbol_name)) {
                    continue;
                }
                if rel_path == file && line.contains(&format!("func {}", symbol_name)) {
                    continue;
                }

                callers.push(CallerInfo {
                    file: rel_path.clone(),
                    line: line_num + 1,
                    text: line.trim().to_string(),
                });

                if callers.len() >= limit {
                    return Ok(callers);
                }
            }
        }
    }

    Ok(callers)
}

#[derive(Debug, serde::Serialize)]
pub struct CallerInfo {
    pub file: String,
    pub line: usize,
    pub text: String,
}

/// Find test functions that reference a given symbol.
pub fn find_tests(
    root: &Path,
    _file_tree: &Arc<FileTree>,
    symbol_table: &Arc<SymbolTable>,
    symbol_name: &str,
    file: &str,
    limit: usize,
) -> Result<Vec<TestInfo>, String> {
    let _sym = symbol_table
        .get(file, symbol_name)
        .ok_or_else(|| format!("Symbol '{}' not found in '{}'", symbol_name, file))?;

    let mut tests = Vec::new();

    // Look through all symbols for test functions
    for entry in symbol_table.symbols.iter() {
        let sym = entry.value();
        if !is_test_symbol(sym) {
            continue;
        }

        // Read the test function body and check if it references the target symbol
        let abs_path = root.join(&sym.file);
        let source = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let start = sym.byte_range.0;
        let end = sym.byte_range.1.min(source.len());
        let body = &source[start..end];

        if body.contains(symbol_name) {
            tests.push(TestInfo {
                name: sym.name.clone(),
                file: sym.file.clone(),
                line: sym.line_range.0,
                signature: sym.signature.clone(),
            });

            if tests.len() >= limit {
                break;
            }
        }
    }

    Ok(tests)
}

fn is_test_symbol(sym: &Symbol) -> bool {
    match sym.language {
        crate::index::file_entry::Language::Rust => {
            // Rust: functions with #[test] attribute — heuristic: name starts with "test" or
            // file is in tests/ dir, or function is in a #[cfg(test)] mod
            sym.name.starts_with("test") || sym.file.contains("/tests/")
        }
        crate::index::file_entry::Language::Python => {
            sym.name.starts_with("test_")
                || sym.file.contains("test_")
                || sym.file.contains("_test.")
        }
        crate::index::file_entry::Language::TypeScript
        | crate::index::file_entry::Language::JavaScript => {
            sym.file.contains(".test.")
                || sym.file.contains(".spec.")
                || sym.file.contains("__tests__")
        }
        crate::index::file_entry::Language::Go => {
            sym.name.starts_with("Test") || sym.file.ends_with("_test.go")
        }
        _ => false,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct TestInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub signature: String,
}

/// List local variables within a function.
pub fn list_variables(
    root: &Path,
    symbol_table: &Arc<SymbolTable>,
    function_name: &str,
    file: &str,
) -> Result<Vec<VariableInfo>, String> {
    let sym = symbol_table
        .get(file, function_name)
        .ok_or_else(|| format!("Symbol '{}' not found in '{}'", function_name, file))?;

    let abs_path = root.join(&sym.file);
    let source = std::fs::read_to_string(&abs_path)
        .map_err(|e| format!("Failed to read '{}': {}", sym.file, e))?;

    let start = sym.byte_range.0;
    let end = sym.byte_range.1.min(source.len());
    let body = &source[start..end];

    let mut variables = Vec::new();

    // Simple heuristic-based variable extraction per language
    match sym.language {
        crate::index::file_entry::Language::Rust => {
            let let_re = regex::Regex::new(r"let\s+(mut\s+)?(\w+)").unwrap();
            for cap in let_re.captures_iter(body) {
                variables.push(VariableInfo {
                    name: cap[2].to_string(),
                    function: function_name.to_string(),
                });
            }
        }
        crate::index::file_entry::Language::Python => {
            let assign_re = regex::Regex::new(r"^\s+(\w+)\s*=").unwrap();
            for cap in assign_re.captures_iter(body) {
                let name = cap[1].to_string();
                if name != "self" && !name.starts_with('_') {
                    variables.push(VariableInfo {
                        name,
                        function: function_name.to_string(),
                    });
                }
            }
        }
        crate::index::file_entry::Language::TypeScript
        | crate::index::file_entry::Language::JavaScript => {
            let var_re = regex::Regex::new(r"(?:let|const|var)\s+(\w+)").unwrap();
            for cap in var_re.captures_iter(body) {
                variables.push(VariableInfo {
                    name: cap[1].to_string(),
                    function: function_name.to_string(),
                });
            }
        }
        crate::index::file_entry::Language::Go => {
            // Short variable declarations (:=) and var statements
            let short_re = regex::Regex::new(r"(\w+)\s*:=").unwrap();
            for cap in short_re.captures_iter(body) {
                variables.push(VariableInfo {
                    name: cap[1].to_string(),
                    function: function_name.to_string(),
                });
            }
            let var_re = regex::Regex::new(r"var\s+(\w+)").unwrap();
            for cap in var_re.captures_iter(body) {
                variables.push(VariableInfo {
                    name: cap[1].to_string(),
                    function: function_name.to_string(),
                });
            }
        }
        _ => {}
    }

    // Deduplicate
    variables.sort_by(|a, b| a.name.cmp(&b.name));
    variables.dedup_by(|a, b| a.name == b.name);

    Ok(variables)
}

#[derive(Debug, serde::Serialize)]
pub struct VariableInfo {
    pub name: String,
    pub function: String,
}
