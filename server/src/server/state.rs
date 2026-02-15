use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::Mutex;
use tracing::info;

use crate::git::github;
use crate::index::file_tree::FileTree;
use crate::index::walker;
use crate::ops::cache;
use crate::server::errors::AppError;
use crate::server::session::Session;
use crate::symbols::{parser, SymbolTable};

/// A single indexed project with its own file tree and symbol table.
pub struct Project {
    pub root: PathBuf,
    pub file_tree: Arc<FileTree>,
    pub symbol_table: Arc<SymbolTable>,
    pub last_active: Mutex<DateTime<Utc>>,
}

/// Shared application state, wrapped in Arc for axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<AppStateInner>,
}

pub struct AppStateInner {
    pub projects: DashMap<PathBuf, Arc<Project>>,
    pub sessions: DashMap<String, Session>,
    pub max_projects: usize,
    pub max_file_size: u64,
    pub repos_dir: PathBuf,
    pub cache_base: Option<PathBuf>,
}

impl AppState {
    pub fn new(
        max_projects: usize,
        max_file_size: u64,
        repos_dir: PathBuf,
        cache_base: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                projects: DashMap::new(),
                sessions: DashMap::new(),
                max_projects,
                max_file_size,
                repos_dir,
                cache_base,
            }),
        }
    }

    /// Look up an existing project or index a new one. Tries to load from
    /// cache first; falls back to full scan + extraction. Evicts LRU if at capacity.
    pub fn get_or_create_project(&self, cwd: &Path) -> Result<Arc<Project>, AppError> {
        let canonical = cwd.canonicalize().map_err(|e| {
            AppError::BadRequest(format!("Path not accessible: {}", e))
        })?;

        if !canonical.is_dir() {
            return Err(AppError::BadRequest(format!(
                "'{}' is not a directory",
                canonical.display()
            )));
        }

        // Return existing project if found
        if let Some(project) = self.inner.projects.get(&canonical) {
            *project.last_active.lock() = Utc::now();
            return Ok(project.clone());
        }

        // Check capacity, evict if needed
        if self.inner.projects.len() >= self.inner.max_projects {
            self.evict_lru()?;
        }

        let file_tree = Arc::new(FileTree::new());
        let symbol_table = Arc::new(SymbolTable::new());
        let max_file_size = self.inner.max_file_size;
        let cache_base = &self.inner.cache_base;

        // Try loading from cache first
        let files_to_extract = match cache::load_index(
            &canonical,
            &file_tree,
            &symbol_table,
            max_file_size,
            cache_base,
        ) {
            Ok(stats) => {
                info!(
                    "Loaded project from cache: {} (cached={}, changed={}, new={}, deleted={})",
                    canonical.display(),
                    stats.cached,
                    stats.changed,
                    stats.new,
                    stats.deleted
                );
                if stats.files_to_extract.is_empty() {
                    None
                } else {
                    Some(stats.files_to_extract)
                }
            }
            Err(reason) => {
                info!(
                    "No usable cache for {}: {}. Doing full index.",
                    canonical.display(),
                    reason
                );
                let file_count =
                    walker::scan_directory(&canonical, &file_tree, max_file_size)
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                info!("Indexed {} files for {}", file_count, canonical.display());
                // None means extract ALL files
                None
            }
        };

        let project = Arc::new(Project {
            root: canonical.clone(),
            file_tree: file_tree.clone(),
            symbol_table: symbol_table.clone(),
            last_active: Mutex::new(Utc::now()),
        });

        self.inner.projects.insert(canonical, project.clone());

        // Spawn symbol extraction in background
        let ft = file_tree;
        let st = symbol_table;
        let root = project.root.clone();
        let only_files = files_to_extract.map(|v| v.into_iter().collect::<HashSet<String>>());
        let cb = cache_base.clone();
        tokio::spawn(async move {
            let scope = if only_files.is_some() { "incremental" } else { "full" };
            info!("Starting {} symbol extraction for {}...", scope, root.display());
            match parser::extract_all_symbols(&root, &ft, &st, only_files, cb).await {
                Ok(count) => info!("Extracted {} symbols for {}", count, root.display()),
                Err(e) => tracing::error!("Symbol extraction failed for {}: {}", root.display(), e),
            }
        });

        Ok(project)
    }

    /// Clone a GitHub repo and index it. Returns the project and (org, repo).
    pub fn clone_and_index(&self, repo_url: &str) -> Result<(Arc<Project>, String, String), AppError> {
        let (org, repo) = github::parse_github_url(repo_url)
            .map_err(AppError::BadRequest)?;

        let target = self.inner.repos_dir.join(&org).join(&repo);

        // Clone if not already present
        if !target.exists() || !github::is_git_repo(&target) {
            github::clone_repo(repo_url, &target)
                .map_err(AppError::Internal)?;
        }

        let project = self.get_or_create_project(&target)?;
        Ok((project, org, repo))
    }

    /// Pull and reindex all projects that live under `repos_dir`.
    pub fn refresh_all_projects(&self) {
        let repos_dir = &self.inner.repos_dir;
        let cache_base = &self.inner.cache_base;
        let max_file_size = self.inner.max_file_size;

        for entry in self.inner.projects.iter() {
            let project = entry.value();
            let root = &project.root;

            // Only pull projects that are inside repos_dir
            if !root.starts_with(repos_dir) {
                continue;
            }

            if !github::is_git_repo(root) {
                continue;
            }

            match github::pull_repo(root) {
                Ok(true) => {
                    info!("Changes detected in {}, reindexing...", root.display());
                    match cache::reindex(
                        root,
                        &project.file_tree,
                        &project.symbol_table,
                        max_file_size,
                        cache_base,
                    ) {
                        Ok(stats) => {
                            if !stats.files_to_extract.is_empty() {
                                let ft = project.file_tree.clone();
                                let st = project.symbol_table.clone();
                                let r = root.clone();
                                let only: HashSet<String> = stats.files_to_extract.into_iter().collect();
                                let cb = cache_base.clone();
                                tokio::spawn(async move {
                                    let _ = parser::extract_all_symbols(&r, &ft, &st, Some(only), cb).await;
                                });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Reindex failed for {}: {}", root.display(), e);
                        }
                    }
                }
                Ok(false) => {
                    tracing::debug!("No changes in {}", root.display());
                }
                Err(e) => {
                    tracing::warn!("Git pull failed for {}: {}", root.display(), e);
                }
            }
        }
    }

    /// Look up the project for a given session. Returns a descriptive error if
    /// the project has been evicted.
    pub fn get_project_for_session(&self, session_id: &str) -> Result<Arc<Project>, AppError> {
        let session = self
            .inner
            .sessions
            .get(session_id)
            .ok_or_else(|| AppError::NotFound(format!("Session '{}' not found", session_id)))?;

        let project_path = &session.project_path;

        let project = self
            .inner
            .projects
            .get(project_path)
            .ok_or_else(|| {
                AppError::Gone(format!(
                    "Project at '{}' was evicted due to capacity limits. \
                     Start a new session to re-index, or increase --max-projects.",
                    project_path.display()
                ))
            })?;

        Ok(project.clone())
    }

    /// Update the last-active timestamp on a project.
    pub fn touch_project(&self, project_path: &Path) {
        if let Some(project) = self.inner.projects.get(project_path) {
            *project.last_active.lock() = Utc::now();
        }
    }

    /// Evict the least recently used project. Saves its index to disk first,
    /// then removes all sessions pointing to it.
    fn evict_lru(&self) -> Result<(), AppError> {
        // Find the project with the oldest last_active
        let oldest = self
            .inner
            .projects
            .iter()
            .min_by_key(|entry| *entry.value().last_active.lock())
            .map(|entry| entry.key().clone());

        let path = oldest.ok_or_else(|| {
            AppError::Internal("No projects to evict".into())
        })?;

        // Save index before evicting
        if let Some(project) = self.inner.projects.get(&path) {
            if let Err(e) = cache::save_index(
                &project.root,
                &project.file_tree,
                &project.symbol_table,
                &self.inner.cache_base,
            ) {
                tracing::warn!("Failed to save index before eviction for {}: {}", path.display(), e);
            }
        }

        info!("Evicting project: {}", path.display());

        self.inner.projects.remove(&path);

        // Remove all sessions attached to this project
        self.inner.sessions.retain(|_, session| session.project_path != path);

        Ok(())
    }

    /// Save indexes for all currently loaded projects. Called on graceful shutdown.
    pub fn save_all_indexes(&self) {
        for entry in self.inner.projects.iter() {
            let project = entry.value();
            if let Err(e) = cache::save_index(
                &project.root,
                &project.file_tree,
                &project.symbol_table,
                &self.inner.cache_base,
            ) {
                tracing::warn!("Failed to save index for {}: {}", project.root.display(), e);
            }
        }
    }
}
