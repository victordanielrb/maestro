use anyhow::{bail, Result};
use std::path::Path;
use tokio::process::Command;

pub async fn add(branch: &str, path: &Path) -> Result<()> {
    add_verbose(branch, path).await.map(|_| ())
}

/// Like `add`, but returns (stdout, stderr) for debug logging.
pub async fn add_verbose(branch: &str, path: &Path) -> Result<(String, String)> {
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch, path.to_str().unwrap_or(".")])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        bail!(
            "exit={} stderr={} stdout={}",
            output.status,
            stderr.trim(),
            stdout.trim()
        );
    }
    Ok((stdout, stderr))
}

pub async fn remove(path: &Path) {
    let _ = Command::new("git")
        .args(["worktree", "remove", "--force", path.to_str().unwrap_or(".")])
        .output()
        .await;
}

/// Returns true if the current directory is inside a git repository.
pub async fn is_git_repo() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Scan .orchestrator/worktrees/ and return paths that git no longer knows about.
pub async fn orphaned_worktrees(base: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };

    let registered = registered_worktree_paths().await;

    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !registered.iter().any(|r| r == p.to_str().unwrap_or("")))
        .collect()
}

async fn registered_worktree_paths() -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output()
        .await
    else {
        return Vec::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(String::from)
        .collect()
}
