use std::sync::Arc;
use std::time::Duration;

use std::process::Stdio;

use anyhow::Result;
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::PlanMode;
use crate::messages::{AppEvent, AppEventTx, TuiCommand, TuiCommandRx};
use crate::task::{Task, TaskStatus};
use crate::worktree;

pub async fn run(
    task: Task,
    event_tx: AppEventTx,
    mut cmd_rx: TuiCommandRx,
) -> Result<()> {
    let task_id = task.id;

    // ── 1. Create worktree ────────────────────────────────────────────────
    send_status(&event_tx, task_id, TaskStatus::CreatingWorktree);
    let branch = format!("agent/{}-{}", task.name, &task_id.to_string()[..8]);

    if let Err(e) = worktree::add(&branch, &task.worktree_path).await {
        let msg = format!("Failed to create worktree: {e}");
        send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
        send_log(&event_tx, task_id, msg);
        let _ = event_tx.send(AppEvent::TaskDone { task_id });
        return Err(e);
    }

    send_log(
        &event_tx,
        task_id,
        format!("Worktree created at {}", task.worktree_path.display()),
    );

    // ── 2. Build command ──────────────────────────────────────────────────
    let mut cmd = Command::new(&task.agent.command);
    cmd.args(&task.agent.args);

    if task.agent.plan_mode == PlanMode::Args {
        cmd.args(&task.agent.plan_args);
        // Prompt as final positional argument
        cmd.arg(&task.prompt);
    }

    cmd.current_dir(&task.worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Redirect stderr to null — inherited stderr would corrupt the TUI
        .stderr(Stdio::null())
        .kill_on_drop(true);

    // ── 3. Spawn ──────────────────────────────────────────────────────────
    let mut child: Child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Failed to spawn '{}': {e}", task.agent.command);
            send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
            send_log(&event_tx, task_id, msg);
            worktree::remove(&task.worktree_path).await;
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Err(e.into());
        }
    };

    let raw_stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");

    let stdin = Arc::new(Mutex::new(BufWriter::new(raw_stdin)));

    // ── 4. Send prompt via stdin (Inline / None modes) ────────────────────
    if task.agent.plan_mode != PlanMode::Args {
        let mut guard = stdin.lock().await;
        if let Some(prefix) = &task.agent.plan_prefix {
            if !prefix.is_empty() {
                guard.write_all(prefix.as_bytes()).await?;
            }
        }
        guard.write_all(task.prompt.as_bytes()).await?;
        guard.write_all(b"\n").await?;
        guard.flush().await?;
    }

    send_status(&event_tx, task_id, TaskStatus::Running);
    info!(task_id = %task_id, agent = %task.agent.name, "Task running");

    // ── 5. Compile plan trigger regex ─────────────────────────────────────
    let plan_trigger: Option<Regex> = task
        .agent
        .plan_trigger
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| Regex::new(s).expect("regex validated at config load"));

    // ── 6. Read stdout line by line ───────────────────────────────────────
    let timeout_enabled = task.agent.input_timeout_secs > 0;
    let timeout_dur = Duration::from_secs(task.agent.input_timeout_secs.max(1) as u64);
    let mut lines = BufReader::new(stdout).lines();

    loop {
        let next_line = lines.next_line();

        let result = if timeout_enabled {
            match tokio::time::timeout(timeout_dur, next_line).await {
                Ok(r) => r,
                Err(_) => {
                    warn!(task_id = %task_id, "stdout timeout after {}s", task.agent.input_timeout_secs);
                    break;
                }
            }
        } else {
            next_line.await
        };

        match result {
            Ok(Some(line)) => {
                send_log(&event_tx, task_id, line.clone());

                if let Some(re) = &plan_trigger {
                    if re.is_match(&line) {
                        info!(task_id = %task_id, "Plan trigger matched, waiting for approval");
                        send_status(&event_tx, task_id, TaskStatus::WaitingApproval);

                        // Wait up to 10 minutes for user decision
                        let decision = tokio::time::timeout(
                            Duration::from_secs(600),
                            cmd_rx.recv(),
                        )
                        .await;

                        match decision {
                            Ok(Some(TuiCommand::AcceptPlan)) => {
                                let mut guard = stdin.lock().await;
                                guard
                                    .write_all(task.agent.plan_accept.as_bytes())
                                    .await?;
                                guard.write_all(b"\n").await?;
                                guard.flush().await?;
                                send_status(&event_tx, task_id, TaskStatus::Executing);
                            }
                            Ok(Some(TuiCommand::RejectPlan)) => {
                                {
                                    let mut guard = stdin.lock().await;
                                    let _ = guard
                                        .write_all(task.agent.plan_reject.as_bytes())
                                        .await;
                                    let _ = guard.write_all(b"\n").await;
                                    let _ = guard.flush().await;
                                }
                                let _ = child.kill().await;
                                send_status(&event_tx, task_id, TaskStatus::Cancelled);
                                cleanup(&task).await;
                                let _ = event_tx.send(AppEvent::TaskDone { task_id });
                                return Ok(());
                            }
                            Ok(Some(TuiCommand::UpdatePrompt(p))) => {
                                let mut guard = stdin.lock().await;
                                guard.write_all(p.as_bytes()).await?;
                                guard.write_all(b"\n").await?;
                                guard.flush().await?;
                                // Resume reading — don't change status
                            }
                            Ok(Some(TuiCommand::KillTask))
                            | Ok(None)
                            | Err(_) => {
                                let _ = child.kill().await;
                                send_status(&event_tx, task_id, TaskStatus::Cancelled);
                                cleanup(&task).await;
                                let _ = event_tx.send(AppEvent::TaskDone { task_id });
                                return Ok(());
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                // EOF — process exited normally
                break;
            }
            Err(e) => {
                warn!(task_id = %task_id, "stdout read error: {e}");
                break;
            }
        }

        // Drain any pending kill commands between lines
        if let Ok(TuiCommand::KillTask) = cmd_rx.try_recv() {
            let _ = child.kill().await;
            send_status(&event_tx, task_id, TaskStatus::Cancelled);
            cleanup(&task).await;
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Ok(());
        }
    }

    // ── 7. Wait for exit status ───────────────────────────────────────────
    match child.wait().await {
        Ok(status) if status.success() => {
            send_status(&event_tx, task_id, TaskStatus::Success);
            info!(task_id = %task_id, "Task succeeded");
        }
        Ok(status) => {
            let msg = format!("Exit code: {:?}", status.code());
            send_status(&event_tx, task_id, TaskStatus::Failed(msg));
        }
        Err(e) => {
            send_status(&event_tx, task_id, TaskStatus::Failed(e.to_string()));
        }
    }

    // ── 8. Post-run script (optional) ─────────────────────────────────────
    if let Some(script) = &task.agent.post_run_script.clone() {
        if !script.is_empty() {
            run_post_script(script, &task.worktree_path).await;
        }
    }

    cleanup(&task).await;
    let _ = event_tx.send(AppEvent::TaskDone { task_id });
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn send_status(tx: &AppEventTx, task_id: Uuid, status: TaskStatus) {
    let _ = tx.send(AppEvent::StatusChange { task_id, status });
}

fn send_log(tx: &AppEventTx, task_id: Uuid, line: String) {
    let _ = tx.send(AppEvent::LogLine { task_id, line });
}

async fn cleanup(task: &Task) {
    worktree::remove(&task.worktree_path).await;
}

async fn run_post_script(script: &str, cwd: &std::path::Path) {
    let result = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .await;

    if let Err(e) = result {
        warn!("post_run_script failed: {e}");
    }
}
