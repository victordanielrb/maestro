use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::messages::{AppEventRx, AppEventTx, TUI_COMMAND_BUFFER};
use crate::tui::app::AppState;
use crate::tui::events::{handle_key, UiAction};

pub mod app;
pub mod chat_widget;
pub mod events;
pub mod ui;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

pub fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

pub async fn run_event_loop(
    terminal: &mut Tui,
    state: &mut AppState,
    app_event_rx: AppEventRx,
    app_event_tx: AppEventTx,
) -> Result<()> {
    // ~60fps tick
    let tick = Duration::from_millis(16);

    loop {
        // ── 1. Drain all pending AppEvents (non-blocking) ─────────────────
        loop {
            match app_event_rx.try_recv() {
                Ok(ev) => state.apply_event(ev),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }

        // ── 2. Render ─────────────────────────────────────────────────────
        terminal.draw(|f| ui::render(f, state))?;

        // ── 3. Poll for input (blocks up to `tick`) ───────────────────────
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match handle_key(key, state) {
                    UiAction::Quit => break,

                    UiAction::ConfirmQuit => {
                        state.kill_all_tasks();
                        // Give controllers a brief moment to receive KillTask
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        break;
                    }

                    UiAction::CancelQuit => {
                        state.show_quit_modal = false;
                    }

                    UiAction::SpawnTask {
                        name,
                        agent_name,
                        mode,
                        prompt,
                    } => {
                        let agent = state.config.find_agent(&agent_name).cloned();
                        match agent {
                            Some(agent) => {
                                // Scope the immutable borrow of add_task before mutably borrowing again
                                let task_id = {
                                    let task = state.add_task(
                                        name.clone(),
                                        agent.clone(),
                                        prompt.clone(),
                                        mode,
                                    );
                                    task.id
                                };

                                // Add user message to task
                                if let Some(t) = state.tasks.iter_mut().find(|t| t.id == task_id) {
                                    t.messages.push(crate::chat::ChatMessage::user(prompt));
                                }

                                let (cmd_tx, cmd_rx) =
                                    mpsc::channel::<crate::messages::TuiCommand>(
                                        TUI_COMMAND_BUFFER,
                                    );
                                state.task_channels.insert(task_id, cmd_tx);

                                let task_clone =
                                    state.tasks.iter().find(|t| t.id == task_id).unwrap().clone();
                                let tx_clone = app_event_tx.clone();
                                tokio::spawn(crate::controller::run(task_clone, tx_clone, cmd_rx));
                            }
                            None => {
                                tracing::warn!("Agent '{}' not found in config", agent_name);
                            }
                        }
                    }

                    UiAction::SendCommand(cmd) => {
                        state.route_command(cmd);
                    }

                    UiAction::Handled | UiAction::Ignored => {}
                }
            }
        }
    }

    Ok(())
}
