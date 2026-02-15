mod config;
mod git;
mod index;
mod ops;
mod server;
mod symbols;

use std::path::PathBuf;

use clap::Parser;
use tracing::info;

use server::state::AppState;

#[derive(Parser)]
#[command(name = "coderlm", about = "CoderLM REPL server for code-aware agent sessions")]
struct Cli {
    /// Subcommand
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Start the REPL server against a codebase
    Serve {
        /// Optional initial project directory to pre-index
        path: Option<PathBuf>,

        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Bind address
        #[arg(short, long, default_value = "127.0.0.1")]
        bind: String,

        /// Maximum file size in bytes to index
        #[arg(long, default_value_t = config::DEFAULT_MAX_FILE_SIZE)]
        max_file_size: u64,

        /// Maximum number of concurrent indexed projects
        #[arg(long, default_value = "5")]
        max_projects: usize,

        /// Directory to clone repos into
        #[arg(long, default_value = "./repos")]
        repos_dir: PathBuf,

        /// Directory to store index cache (default: .coderlm inside each project)
        #[arg(long)]
        cache_dir: Option<PathBuf>,

        /// Interval in seconds for periodic git pull + reindex (0 = disabled)
        #[arg(long, default_value = "3600")]
        refresh_interval: u64,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("coderlm v{}", env!("CARGO_PKG_VERSION"));

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            path,
            port,
            bind,
            max_file_size,
            max_projects,
            repos_dir,
            cache_dir,
            refresh_interval,
        } => {
            run_server(path, port, bind, max_file_size, max_projects, repos_dir, cache_dir, refresh_interval).await?;
        }
    }

    Ok(())
}

async fn run_server(
    path: Option<PathBuf>,
    port: u16,
    bind: String,
    max_file_size: u64,
    max_projects: usize,
    repos_dir: PathBuf,
    cache_dir: Option<PathBuf>,
    refresh_interval: u64,
) -> anyhow::Result<()> {
    // Ensure repos_dir exists
    std::fs::create_dir_all(&repos_dir)?;
    let repos_dir = repos_dir.canonicalize()?;

    // Ensure cache_dir exists if specified
    let cache_base = if let Some(ref dir) = cache_dir {
        std::fs::create_dir_all(dir)?;
        Some(dir.canonicalize()?)
    } else {
        None
    };

    info!("Repos dir: {}", repos_dir.display());
    if let Some(ref cb) = cache_base {
        info!("Cache dir: {}", cb.display());
    }
    if refresh_interval > 0 {
        info!("Refresh interval: {}s", refresh_interval);
    }

    // Create shared state
    let state = AppState::new(max_projects, max_file_size, repos_dir, cache_base);

    // If an initial path was provided, pre-index it
    if let Some(ref p) = path {
        info!("Pre-indexing project: {}", p.display());
        state.get_or_create_project(p).map_err(|e| {
            anyhow::anyhow!("Failed to index '{}': {}", p.display(), e)
        })?;
    }

    // Spawn periodic refresh loop
    if refresh_interval > 0 {
        let refresh_state = state.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(refresh_interval);
            loop {
                tokio::time::sleep(interval).await;
                info!("Running periodic refresh...");
                refresh_state.refresh_all_projects();
                info!("Periodic refresh complete.");
            }
        });
    }

    // Build router
    let state_for_shutdown = state.clone();
    let app = server::build_router(state);

    let addr = format!("{}:{}", bind, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    if let Some(ref p) = path {
        info!("coderlm serving {} on http://{}", p.display(), addr);
    } else {
        info!("coderlm server listening on http://{} (no project pre-indexed)", addr);
    }

    // Graceful shutdown: save all indexes on ctrl-c
    let shutdown_state = state_for_shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutting down — saving indexes...");
            shutdown_state.save_all_indexes();
            info!("Indexes saved. Goodbye.");
        })
        .await?;

    Ok(())
}
