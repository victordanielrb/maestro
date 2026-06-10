use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
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

    debug(&event_tx, format!(
        "[controller] started task={} agent={} mode={:?} cwd={}",
        task.name, task.agent.command, task.agent.plan_mode, task.worktree_path.display()
    ));

    // ── 1. Create worktree ────────────────────────────────────────────────
    send_status(&event_tx, task_id, TaskStatus::CreatingWorktree);
    let branch = format!("agent/{}-{}", task.name, &task_id.to_string()[..8]);

    debug(&event_tx, format!("[worktree] git worktree add -b {} {}", branch, task.worktree_path.display()));

    match worktree::add_verbose(&branch, &task.worktree_path).await {
        Ok((stdout, stderr)) => {
            if !stdout.trim().is_empty() {
                debug(&event_tx, format!("[worktree stdout] {}", stdout.trim()));
            }
            if !stderr.trim().is_empty() {
                debug(&event_tx, format!("[worktree stderr] {}", stderr.trim()));
            }
            debug(&event_tx, "[worktree] created OK".into());
        }
        Err(e) => {
            let msg = format!("Failed to create worktree: {e}");
            debug(&event_tx, format!("[worktree] FAILED: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
            send_log(&event_tx, task_id, msg);
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Err(e);
        }
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
        cmd.arg(&task.prompt);
    }

    let full_cmd = format!(
        "{} {}",
        task.agent.command,
        task.agent.args.join(" ")
    );
    debug(&event_tx, format!("[spawn] cmd={} cwd={}", full_cmd, task.worktree_path.display()));

    cmd.current_dir(&task.worktree_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Capture stderr — pipe to debug log instead of /dev/null
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // ── 3. Spawn ──────────────────────────────────────────────────────────
    let mut child: Child = match cmd.spawn() {
        Ok(c) => {
            debug(&event_tx, format!("[spawn] OK pid={:?}", c.id()));
            c
        }
        Err(e) => {
            let msg = format!("Failed to spawn '{}': {e}", task.agent.command);
            debug(&event_tx, format!("[spawn] FAILED: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
            send_log(&event_tx, task_id, msg);
            worktree::remove(&task.worktree_path).await;
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Err(e.into());
        }
    };

    let raw_stdin = child.stdin.take().expect("stdin was piped");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let stdin = Arc::new(Mutex::new(BufWriter::new(raw_stdin)));

    // Drain stderr in a background task and send to debug log
    {
        let tx = event_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(AppEvent::Debug(format!("[stderr] {}", line)));
            }
        });
    }

    // ── 4. Send prompt via stdin (Inline / None modes) ────────────────────
    if task.agent.plan_mode != PlanMode::Args {
        debug(&event_tx, format!("[stdin] writing prompt ({} chars)", task.prompt.len()));
        let mut guard = stdin.lock().await;
        if let Some(prefix) = &task.agent.plan_prefix {
            if !prefix.is_empty() {
                guard.write_all(prefix.as_bytes()).await?;
            }
        }
        guard.write_all(task.prompt.as_bytes()).await?;
        guard.write_all(b"\n").await?;
        guard.flush().await?;
        debug(&event_tx, "[stdin] prompt flushed".into());
    }

    send_status(&event_tx, task_id, TaskStatus::Running);
    debug(&event_tx, "[controller] status=Running, reading stdout...".into());

    // ── 5. Compile plan trigger regex ─────────────────────────────────────
    let plan_trigger: Option<Regex> = task
        .agent
        .plan_trigger
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| Regex::new(s).expect("regex validated at config load"));

    if let Some(re) = &plan_trigger {
        debug(&event_tx, format!("[trigger] watching for regex: {}", re.as_str()));
    } else {
        debug(&event_tx, "[trigger] no plan_trigger configured".into());
    }

    // ── 6. Read stdout line by line ───────────────────────────────────────
    let timeout_enabled = task.agent.input_timeout_secs > 0;
    let timeout_dur = Duration::from_secs(task.agent.input_timeout_secs.max(1) as u64);
    let mut lines = BufReader::new(stdout).lines();
    let mut line_count: u32 = 0;

    loop {
        let next_line = lines.next_line();

        let result = if timeout_enabled {
            match tokio::time::timeout(timeout_dur, next_line).await {
                Ok(r) => r,
                Err(_) => {
                    debug(&event_tx, format!("[stdout] timeout after {}s (no output)", task.agent.input_timeout_secs));
                    break;
                }
            }
        } else {
            next_line.await
        };

        match result {
            Ok(Some(line)) => {
                line_count += 1;
                if line_count <= 5 || line_count % 50 == 0 {
                    debug(&event_tx, format!("[stdout:{}] {}", line_count, &line[..line.len().min(80)]));
                }
                send_log(&event_tx, task_id, line.clone());

                if let Some(re) = &plan_trigger {
                    if re.is_match(&line) {
                        debug(&event_tx, format!("[trigger] MATCHED on line {}: {:?}", line_count, &line[..line.len().min(60)]));
                        send_status(&event_tx, task_id, TaskStatus::WaitingApproval);

                        let decision = tokio::time::timeout(
                            Duration::from_secs(600),
                            cmd_rx.recv(),
                        )
                        .await;

                        match decision {
                            Ok(Some(TuiCommand::AcceptPlan)) => {
                                debug(&event_tx, "[plan] accepted — writing to stdin".into());
                                let mut guard = stdin.lock().await;
                                guard.write_all(task.agent.plan_accept.as_bytes()).await?;
                                guard.write_all(b"\n").await?;
                                guard.flush().await?;
                                send_status(&event_tx, task_id, TaskStatus::Executing);
                            }
                            Ok(Some(TuiCommand::RejectPlan)) => {
                                debug(&event_tx, "[plan] rejected".into());
                                {
                                    let mut guard = stdin.lock().await;
                                    let _ = guard.write_all(task.agent.plan_reject.as_bytes()).await;
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
                                debug(&event_tx, format!("[plan] update prompt: {:?}", &p[..p.len().min(40)]));
                                let mut guard = stdin.lock().await;
                                guard.write_all(p.as_bytes()).await?;
                                guard.write_all(b"\n").await?;
                                guard.flush().await?;
                            }
                            Ok(Some(TuiCommand::KillTask)) | Ok(None) | Err(_) => {
                                debug(&event_tx, "[plan] killed/timeout while waiting for approval".into());
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
                debug(&event_tx, format!("[stdout] EOF after {} lines", line_count));
                break;
            }
            Err(e) => {
                debug(&event_tx, format!("[stdout] read error: {e}"));
                break;
            }
        }

        if let Ok(TuiCommand::KillTask) = cmd_rx.try_recv() {
            debug(&event_tx, "[controller] KillTask received".into());
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
            debug(&event_tx, "[controller] exit OK".into());
            send_status(&event_tx, task_id, TaskStatus::Success);
        }
        Ok(status) => {
            let msg = format!("Exit code: {:?}", status.code());
            debug(&event_tx, format!("[controller] exit FAILED: {msg}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg));
        }
        Err(e) => {
            debug(&event_tx, format!("[controller] wait error: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(e.to_string()));
        }
    }

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

fn debug(tx: &AppEventTx, msg: String) {
    let ts = Local::now().format("%H:%M:%S%.3f");
    let _ = tx.send(AppEvent::Debug(format!("{} {}", ts, msg)));
}

async fn cleanup(task: &Task) {
    worktree::remove(&task.worktree_path).await;
}

async fn run_post_script(script: &str, cwd: &std::path::Path) {
    if let Err(e) = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(cwd)
        .output()
        .await
    {
        eprintln!("post_run_script failed: {e}");
    }
}
