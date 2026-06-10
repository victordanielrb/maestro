use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::messages::TuiCommand;
use crate::task::ExecutionMode;
use crate::tui::app::{AppState, Focus, FormField, NewTaskForm};

#[derive(Debug)]
pub enum UiAction {
    SpawnTask {
        name: String,
        agent_name: String,
        mode: ExecutionMode,
        prompt: String,
    },
    SendCommand(TuiCommand),
    Quit,
    ConfirmQuit,
    CancelQuit,
    Handled,
    Ignored,
}

pub fn handle_key(key: KeyEvent, state: &mut AppState) -> UiAction {
    // ── Debug overlay (highest priority — always closeable) ───────────────
    if state.show_debug {
        return handle_debug_overlay(key, state);
    }

    // ── Quit modal ────────────────────────────────────────────────────────
    if state.show_quit_modal {
        return handle_quit_modal(key, state);
    }

    // ── Inline input (UpdatePrompt edit) ──────────────────────────────────
    if state.inline_input.is_some() {
        return handle_inline_input(key, state);
    }

    // ── New task form ─────────────────────────────────────────────────────
    if state.pending_form.is_some() {
        return handle_form(key, state);
    }

    // ── Global keys ───────────────────────────────────────────────────────
    match key.code {
        KeyCode::Char('q') => {
            if state.has_active_tasks() {
                state.show_quit_modal = true;
                return UiAction::Handled;
            }
            return UiAction::Quit;
        }
        KeyCode::Char('n') => {
            state.pending_form = Some(NewTaskForm::new());
            state.focus = Focus::NewTaskForm;
            return UiAction::Handled;
        }
        KeyCode::Tab => {
            state.focus = match state.focus {
                Focus::TaskList => Focus::Chat,
                Focus::Chat => Focus::TaskList,
                Focus::NewTaskForm => Focus::NewTaskForm,
            };
            return UiAction::Handled;
        }
        KeyCode::Char('?') | KeyCode::Char('L') => {
            state.toggle_debug();
            return UiAction::Handled;
        }
        _ => {}
    }

    // ── Focus-specific keys ───────────────────────────────────────────────
    match state.focus {
        Focus::TaskList => handle_task_list(key, state),
        Focus::Chat => handle_chat(key, state),
        Focus::NewTaskForm => UiAction::Ignored,
    }
}

fn handle_debug_overlay(key: KeyEvent, state: &mut AppState) -> UiAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            state.show_debug = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.debug_scroll = state.debug_scroll.saturating_add(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.debug_scroll = state.debug_scroll.saturating_sub(1);
        }
        KeyCode::Char('g') => {
            state.debug_scroll = usize::MAX / 2;
        }
        KeyCode::Char('G') => {
            state.debug_scroll = 0;
        }
        _ => {}
    }
    UiAction::Handled
}

fn handle_quit_modal(key: KeyEvent, state: &mut AppState) -> UiAction {
    match key.code {
        KeyCode::Char('s') | KeyCode::Char('y') | KeyCode::Enter => {
            state.show_quit_modal = false;
            UiAction::ConfirmQuit
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            state.show_quit_modal = false;
            UiAction::CancelQuit
        }
        _ => UiAction::Handled,
    }
}

fn handle_inline_input(key: KeyEvent, state: &mut AppState) -> UiAction {
    match key.code {
        KeyCode::Enter => {
            let prompt = state.inline_input.take().unwrap_or_default();
            UiAction::SendCommand(TuiCommand::UpdatePrompt(prompt))
        }
        KeyCode::Esc => {
            state.inline_input = None;
            UiAction::Handled
        }
        KeyCode::Backspace => {
            if let Some(buf) = &mut state.inline_input {
                buf.pop();
            }
            UiAction::Handled
        }
        KeyCode::Char(c) => {
            if let Some(buf) = &mut state.inline_input {
                buf.push(c);
            }
            UiAction::Handled
        }
        _ => UiAction::Handled,
    }
}

fn handle_form(key: KeyEvent, state: &mut AppState) -> UiAction {
    let form = state.pending_form.as_mut().unwrap();
    let agent_count = state.config.agents.len();

    match key.code {
        KeyCode::Esc => {
            state.pending_form = None;
            state.focus = Focus::TaskList;
            return UiAction::Handled;
        }
        KeyCode::Tab | KeyCode::Down => {
            form.active_field = match form.active_field {
                FormField::Name => FormField::Agent,
                FormField::Agent => FormField::Mode,
                FormField::Mode => FormField::Prompt,
                FormField::Prompt => FormField::Name,
            };
            return UiAction::Handled;
        }
        KeyCode::BackTab | KeyCode::Up => {
            form.active_field = match form.active_field {
                FormField::Name => FormField::Prompt,
                FormField::Agent => FormField::Name,
                FormField::Mode => FormField::Agent,
                FormField::Prompt => FormField::Mode,
            };
            return UiAction::Handled;
        }
        KeyCode::Enter => {
            match form.active_field {
                FormField::Prompt => {
                    // Submit form
                    if form.name.is_empty() || form.prompt.is_empty() {
                        return UiAction::Handled;
                    }
                    let name = form.name.clone();
                    let agent_idx = form.agent_idx;
                    let mode = form.mode.clone();
                    let prompt = form.prompt.clone();
                    let agent_name = state
                        .config
                        .agents
                        .get(agent_idx)
                        .map(|a| a.name.clone())
                        .unwrap_or_default();
                    state.pending_form = None;
                    state.focus = Focus::TaskList;
                    return UiAction::SpawnTask {
                        name,
                        agent_name,
                        mode,
                        prompt,
                    };
                }
                FormField::Agent => {
                    form.agent_idx = (form.agent_idx + 1) % agent_count.max(1);
                    return UiAction::Handled;
                }
                FormField::Mode => {
                    form.mode = match form.mode {
                        ExecutionMode::Plan => ExecutionMode::Direct,
                        ExecutionMode::Direct => ExecutionMode::Plan,
                    };
                    return UiAction::Handled;
                }
                _ => {
                    form.active_field = match form.active_field {
                        FormField::Name => FormField::Agent,
                        FormField::Agent => FormField::Mode,
                        FormField::Mode => FormField::Prompt,
                        FormField::Prompt => FormField::Name,
                    };
                    return UiAction::Handled;
                }
            }
        }
        KeyCode::Backspace => {
            match form.active_field {
                FormField::Name => { form.name.pop(); }
                FormField::Prompt => { form.prompt.pop(); }
                _ => {}
            }
            return UiAction::Handled;
        }
        KeyCode::Char(c) => {
            match form.active_field {
                FormField::Name => form.name.push(c),
                FormField::Prompt => form.prompt.push(c),
                FormField::Agent => {
                    if c == 'h' || c == 'k' {
                        if form.agent_idx > 0 { form.agent_idx -= 1; }
                    } else if c == 'l' || c == 'j' {
                        form.agent_idx = (form.agent_idx + 1) % agent_count.max(1);
                    }
                }
                FormField::Mode => {
                    if c == ' ' || c == 'l' || c == 'j' {
                        form.mode = match form.mode {
                            ExecutionMode::Plan => ExecutionMode::Direct,
                            ExecutionMode::Direct => ExecutionMode::Plan,
                        };
                    }
                }
            }
            return UiAction::Handled;
        }
        _ => {}
    }
    UiAction::Handled
}

fn handle_task_list(key: KeyEvent, state: &mut AppState) -> UiAction {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.navigate_up();
            UiAction::Handled
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.navigate_down();
            UiAction::Handled
        }
        KeyCode::Enter => {
            state.focus = Focus::Chat;
            UiAction::Handled
        }
        _ => UiAction::Ignored,
    }
}

fn handle_chat(key: KeyEvent, state: &mut AppState) -> UiAction {
    // Ctrl+k must be checked before bare 'k' to avoid being shadowed
    if key.code == KeyCode::Char('k') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return UiAction::SendCommand(TuiCommand::KillTask);
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.chat_scroll = state.chat_scroll.saturating_add(1);
            UiAction::Handled
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.chat_scroll = state.chat_scroll.saturating_sub(1);
            UiAction::Handled
        }
        KeyCode::Char('g') => {
            state.chat_scroll = usize::MAX / 2;
            UiAction::Handled
        }
        KeyCode::Char('G') => {
            state.chat_scroll = 0;
            UiAction::Handled
        }
        KeyCode::Char('a') => UiAction::SendCommand(TuiCommand::AcceptPlan),
        KeyCode::Char('r') => UiAction::SendCommand(TuiCommand::RejectPlan),
        KeyCode::Char('e') => {
            state.inline_input = Some(String::new());
            UiAction::Handled
        }
        _ => UiAction::Ignored,
    }
}
