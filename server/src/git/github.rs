use std::path::Path;
use std::process::Command;

use tracing::info;

/// Parse a GitHub URL into (org, repo).
/// Supports:
///   - https://github.com/org/repo
///   - https://github.com/org/repo.git
///   - git@github.com:org/repo.git
pub fn parse_github_url(url: &str) -> Result<(String, String), String> {
    // Try HTTPS format
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return parse_org_repo(rest);
    }

    // Try SSH format
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return parse_org_repo(rest);
    }

    Err(format!(
        "Unsupported URL format: {}. Expected https://github.com/org/repo or git@github.com:org/repo.git",
        url
    ))
}

fn parse_org_repo(path: &str) -> Result<(String, String), String> {
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = path.splitn(3, '/').collect();
    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("Could not extract org/repo from path: {}", path));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Clone a repo with `--depth 1` into the target directory.
pub fn clone_repo(url: &str, target: &Path) -> Result<(), String> {
    info!("Cloning {} into {}", url, target.display());

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent dir: {}", e))?;
    }

    let output = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(target)
        .output()
        .map_err(|e| format!("Failed to run git clone: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git clone failed: {}", stderr.trim()));
    }

    info!("Cloned {} successfully", url);
    Ok(())
}

/// Pull latest changes in a repo directory. Returns whether files changed.
pub fn pull_repo(path: &Path) -> Result<bool, String> {
    // First fetch
    let output = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Failed to run git pull: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git pull failed in {}: {}", path.display(), stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // "Already up to date." means no changes
    let changed = !stdout.contains("Already up to date");
    Ok(changed)
}

/// Check if a directory is a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}
