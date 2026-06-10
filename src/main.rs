mod chat;
mod config;
mod controller;
mod messages;
mod task;
mod tui;
mod worktree;

use std::path::PathBuf;

use anyhow::{bail, Result};
use tracing::info;

const AGENTS_CONFIG: &str = "agents.toml";
const ORCHESTRATOR_DIR: &str = ".orchestrator";
const WORKTREES_DIR: &str = ".orchestrator/worktrees";
const LOG_FILE: &str = ".orchestrator/orchestrator.log";

#[tokio::main]
async fn main() -> Result<()> {
    // ── 1. Validate git repo (fail fast before touching terminal) ─────────
    if !worktree::is_git_repo().await {
        bail!(
            "Not inside a git repository.\n\
             agent-orchestrator must be run from the root of a git repo."
        );
    }

    // ── 2. Ensure .orchestrator/ dirs exist ───────────────────────────────
    std::fs::create_dir_all(WORKTREES_DIR)?;

    // ── 3. Initialize tracing to FILE — never stderr (would corrupt TUI) ──
    let log_file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(LOG_FILE)?;

    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter(
            std::env::var("ORCHESTRATOR_LOG")
                .unwrap_or_else(|_| "info".to_string()),
        )
        .init();

    info!("agent-orchestrator starting");

    // ── 4. Load agents config ─────────────────────────────────────────────
    let config = config::load(AGENTS_CONFIG)?;
    info!("Loaded {} agent profile(s)", config.agents.len());

    // ── 5. Warn about orphaned worktrees (non-fatal) ──────────────────────
    let orphans = worktree::orphaned_worktrees(&PathBuf::from(WORKTREES_DIR)).await;
    if !orphans.is_empty() {
        tracing::warn!(
            "{} orphaned worktree(s) found in {}. Run `git worktree prune` to clean up.",
            orphans.len(),
            WORKTREES_DIR
        );
    }

    // ── 6. Create AppEvent channel ────────────────────────────────────────
    let (app_event_tx, app_event_rx) = std::sync::mpsc::channel::<messages::AppEvent>();

    // ── 7. Build AppState ─────────────────────────────────────────────────
    let mut state = tui::app::AppState::new(config, PathBuf::from(WORKTREES_DIR));

    // ── 8. Setup terminal ─────────────────────────────────────────────────
    let mut terminal = tui::setup_terminal()?;

    // ── 9. Run TUI event loop ─────────────────────────────────────────────
    let result = tui::run_event_loop(&mut terminal, &mut state, app_event_rx, app_event_tx).await;

    // ── 10. Restore terminal (always, even on error) ──────────────────────
    tui::restore_terminal(&mut terminal)?;

    if let Err(ref e) = result {
        eprintln!("Error: {e:?}");
    }

    info!("agent-orchestrator exiting");
    result
}
