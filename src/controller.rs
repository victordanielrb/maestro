use std::io::{BufRead, BufReader as StdBufReader, Write as StdWrite};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use regex::Regex;
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

    send_log(&event_tx, task_id, format!("Worktree created at {}", task.worktree_path.display()));

    // ── 2. Open PTY ───────────────────────────────────────────────────────
    // Claude (and most interactive CLIs) detect they're not on a TTY via
    // isatty() and disable output buffering / colors / interactivity.
    // Spawning inside a PTY makes the child believe it has a real terminal.
    let pty_system = native_pty_system();
    let pty_pair = match pty_system.openpty(PtySize {
        rows: 50,
        cols: 220,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Failed to open PTY: {e}");
            debug(&event_tx, format!("[pty] FAILED: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
            send_log(&event_tx, task_id, msg);
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Err(anyhow::anyhow!(e));
        }
    };

    // ── 3. Build and spawn command in PTY slave ───────────────────────────
    let mut cmd_builder = CommandBuilder::new(&task.agent.command);
    cmd_builder.args(&task.agent.args);

    if task.agent.plan_mode == PlanMode::Args {
        cmd_builder.args(&task.agent.plan_args);
        cmd_builder.arg(&task.prompt);
    }

    if let Some(cwd) = task.worktree_path.to_str() {
        cmd_builder.cwd(cwd);
    }

    let full_cmd = format!("{} {}", task.agent.command, task.agent.args.join(" "));
    debug(&event_tx, format!("[spawn] PTY cmd={} cwd={}", full_cmd, task.worktree_path.display()));

    let child = match pty_pair.slave.spawn_command(cmd_builder) {
        Ok(c) => {
            debug(&event_tx, format!("[spawn] OK pid={:?}", c.process_id()));
            c
        }
        Err(e) => {
            let msg = format!("Failed to spawn '{}': {e}", task.agent.command);
            debug(&event_tx, format!("[spawn] FAILED: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg.clone()));
            send_log(&event_tx, task_id, msg);
            drop(pty_pair.slave);
            worktree::remove(&task.worktree_path).await;
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Err(anyhow::anyhow!(e));
        }
    };

    // Drop slave end in the parent after spawning — important for proper EOF
    drop(pty_pair.slave);

    let child = Arc::new(Mutex::new(child));

    // ── 4. Get PTY reader/writer ──────────────────────────────────────────
    let pty_reader = match pty_pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            debug(&event_tx, format!("[pty] reader failed: {e}"));
            return Err(anyhow::anyhow!(e));
        }
    };

    let pty_writer: Arc<Mutex<Box<dyn StdWrite + Send>>> = Arc::new(Mutex::new(
        pty_pair.master.take_writer().map_err(|e| anyhow::anyhow!(e))?,
    ));

    // ── 5. Write prompt to PTY stdin ──────────────────────────────────────
    if task.agent.plan_mode != PlanMode::Args {
        debug(&event_tx, format!("[stdin] writing prompt ({} chars)", task.prompt.len()));
        let mut w = pty_writer.lock().unwrap();
        if let Some(prefix) = &task.agent.plan_prefix {
            if !prefix.is_empty() {
                let _ = write!(w, "{}", prefix);
            }
        }
        let _ = writeln!(w, "{}", task.prompt);
        let _ = w.flush();
        debug(&event_tx, "[stdin] prompt flushed".into());
    }

    send_status(&event_tx, task_id, TaskStatus::Running);
    debug(&event_tx, "[controller] status=Running, reading PTY stdout...".into());

    // ── 6. Spawn blocking reader → tokio channel ──────────────────────────
    // PTY reads are blocking; we bridge them to async via a channel.
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(512);
    {
        let tx = line_tx;
        tokio::task::spawn_blocking(move || {
            let reader = StdBufReader::new(pty_reader);
            for line in reader.lines() {
                match line {
                    Ok(raw) => {
                        // Strip ANSI escape codes — PTY gives us colored output
                        let clean = strip_ansi_escapes::strip_str(&raw);
                        let clean = clean.trim_end_matches('\r').to_string();
                        if tx.blocking_send(clean).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // EOF — master closed or child exited
        });
    }

    // ── 7. Compile plan trigger regex ─────────────────────────────────────
    let plan_trigger: Option<Regex> = task
        .agent
        .plan_trigger
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| Regex::new(s).expect("regex validated at config load"));

    if let Some(re) = &plan_trigger {
        debug(&event_tx, format!("[trigger] watching for: {}", re.as_str()));
    }

    let timeout_enabled = task.agent.input_timeout_secs > 0;
    let timeout_dur = Duration::from_secs(task.agent.input_timeout_secs.max(1) as u64);
    let mut line_count: u32 = 0;

    // ── 8. Main read loop ─────────────────────────────────────────────────
    loop {
        // Receive next line, with optional timeout
        let received = if timeout_enabled {
            match tokio::time::timeout(timeout_dur, line_rx.recv()).await {
                Ok(v) => v,
                Err(_) => {
                    debug(&event_tx, format!("[stdout] timeout after {}s", task.agent.input_timeout_secs));
                    break;
                }
            }
        } else {
            line_rx.recv().await
        };

        match received {
            Some(line) => {
                line_count += 1;
                // Log first lines + every 50th to debug without flooding
                if line_count <= 10 || line_count % 50 == 0 {
                    debug(&event_tx, format!("[stdout:{}] {}", line_count, line.chars().take(100).collect::<String>().as_str()));
                }

                // Skip echo of our own writes (PTY echoes stdin back on stdout)
                // Heuristic: skip very short lines that match what we sent
                let is_likely_echo = line.trim() == task.agent.plan_accept.trim()
                    || line.trim() == task.agent.plan_reject.trim();
                if !is_likely_echo {
                    send_log(&event_tx, task_id, line.clone());
                }

                if let Some(re) = &plan_trigger {
                    if re.is_match(&line) {
                        debug(&event_tx, format!(
                            "[trigger] MATCHED line {}: {:?}",
                            line_count,
                            line.chars().take(80).collect::<String>().as_str()
                        ));
                        send_status(&event_tx, task_id, TaskStatus::WaitingApproval);

                        let decision = tokio::time::timeout(
                            Duration::from_secs(600),
                            cmd_rx.recv(),
                        ).await;

                        match decision {
                            Ok(Some(TuiCommand::AcceptPlan)) => {
                                debug(&event_tx, "[plan] accepted".into());
                                let mut w = pty_writer.lock().unwrap();
                                let _ = writeln!(w, "{}", task.agent.plan_accept);
                                let _ = w.flush();
                                send_status(&event_tx, task_id, TaskStatus::Executing);
                            }
                            Ok(Some(TuiCommand::RejectPlan)) => {
                                debug(&event_tx, "[plan] rejected".into());
                                {
                                    let mut w = pty_writer.lock().unwrap();
                                    let _ = writeln!(w, "{}", task.agent.plan_reject);
                                    let _ = w.flush();
                                }
                                kill_child(&child);
                                send_status(&event_tx, task_id, TaskStatus::Cancelled);
                                cleanup(&task).await;
                                let _ = event_tx.send(AppEvent::TaskDone { task_id });
                                return Ok(());
                            }
                            Ok(Some(TuiCommand::UpdatePrompt(p))) => {
                                debug(&event_tx, format!("[plan] update prompt ({} chars)", p.len()));
                                let mut w = pty_writer.lock().unwrap();
                                let _ = writeln!(w, "{}", p);
                                let _ = w.flush();
                            }
                            Ok(Some(TuiCommand::KillTask)) | Ok(None) | Err(_) => {
                                debug(&event_tx, "[plan] killed/timed out while waiting".into());
                                kill_child(&child);
                                send_status(&event_tx, task_id, TaskStatus::Cancelled);
                                cleanup(&task).await;
                                let _ = event_tx.send(AppEvent::TaskDone { task_id });
                                return Ok(());
                            }
                        }
                    }
                }
            }
            None => {
                debug(&event_tx, format!("[stdout] EOF after {} lines", line_count));
                break;
            }
        }

        // Check for kill between lines
        if let Ok(TuiCommand::KillTask) = cmd_rx.try_recv() {
            debug(&event_tx, "[controller] KillTask".into());
            kill_child(&child);
            send_status(&event_tx, task_id, TaskStatus::Cancelled);
            cleanup(&task).await;
            let _ = event_tx.send(AppEvent::TaskDone { task_id });
            return Ok(());
        }
    }

    // ── 9. Wait for exit ──────────────────────────────────────────────────
    let child_arc = Arc::clone(&child);
    let exit_result = tokio::task::spawn_blocking(move || {
        child_arc.lock().unwrap().wait()
    }).await;

    match exit_result {
        Ok(Ok(status)) if status.success() => {
            debug(&event_tx, "[controller] exit OK".into());
            send_status(&event_tx, task_id, TaskStatus::Success);
        }
        Ok(Ok(status)) => {
            let msg = format!("Exit code: {:?}", status.exit_code());
            debug(&event_tx, format!("[controller] exit FAILED: {msg}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(msg));
        }
        Ok(Err(e)) => {
            debug(&event_tx, format!("[controller] wait error: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed(e.to_string()));
        }
        Err(e) => {
            debug(&event_tx, format!("[controller] join error: {e}"));
            send_status(&event_tx, task_id, TaskStatus::Failed("task panic".into()));
        }
    }

    if let Some(script) = &task.agent.post_run_script.clone() {
        if !script.is_empty() {
            tokio::process::Command::new("sh")
                .arg("-c").arg(script)
                .current_dir(&task.worktree_path)
                .output().await.ok();
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

fn kill_child(child: &Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>) {
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
    }
}

async fn cleanup(task: &Task) {
    worktree::remove(&task.worktree_path).await;
}
