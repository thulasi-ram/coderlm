pub mod parser;
pub mod queries;
pub mod symbol;

use dashmap::DashMap;
use std::collections::HashSet;

use symbol::Symbol;

/// Thread-safe symbol table with secondary indices for fast lookup.
pub struct SymbolTable {
    /// Primary store: keyed by "file::name"
    pub symbols: DashMap<String, Symbol>,
    /// Secondary index: symbol name -> set of primary keys
    pub by_name: DashMap<String, HashSet<String>>,
    /// Secondary index: file path -> set of primary keys
    pub by_file: DashMap<String, HashSet<String>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            symbols: DashMap::new(),
            by_name: DashMap::new(),
            by_file: DashMap::new(),
        }
    }

    pub fn make_key(file: &str, name: &str) -> String {
        format!("{}::{}", file, name)
    }

    pub fn insert(&self, symbol: Symbol) {
        let key = Self::make_key(&symbol.file, &symbol.name);

        // Update secondary indices
        self.by_name
            .entry(symbol.name.clone())
            .or_insert_with(HashSet::new)
            .insert(key.clone());
        self.by_file
            .entry(symbol.file.clone())
            .or_insert_with(HashSet::new)
            .insert(key.clone());

        self.symbols.insert(key, symbol);
    }

    pub fn remove_file(&self, file: &str) {
        if let Some((_, keys)) = self.by_file.remove(file) {
            for key in &keys {
                if let Some((_, sym)) = self.symbols.remove(key) {
                    if let Some(mut name_set) = self.by_name.get_mut(&sym.name) {
                        name_set.remove(key);
                        if name_set.is_empty() {
                            drop(name_set);
                            self.by_name.remove(&sym.name);
                        }
                    }
                }
            }
        }
    }

    pub fn get(&self, file: &str, name: &str) -> Option<Symbol> {
        let key = Self::make_key(file, name);
        self.symbols.get(&key).map(|r| r.value().clone())
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();
        for entry in self.symbols.iter() {
            if entry.value().name.to_lowercase().contains(&query_lower) {
                results.push(entry.value().clone());
                if results.len() >= limit {
                    break;
                }
            }
        }
        results
    }

    pub fn list_by_file(&self, file: &str) -> Vec<Symbol> {
        if let Some(keys) = self.by_file.get(file) {
            keys.iter()
                .filter_map(|key| self.symbols.get(key).map(|r| r.value().clone()))
                .collect()
        } else {
            Vec::new()
        }
    }

    pub fn all_symbols(&self) -> Vec<Symbol> {
        self.symbols.iter().map(|r| r.value().clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }
}
