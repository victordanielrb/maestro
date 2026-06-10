use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::task::TaskStatus;
use crate::tui::app::{AppState, Focus, FormField};
use crate::tui::chat_widget::ChatWidget;

pub fn render(f: &mut Frame, state: &mut AppState) {
    let area = f.area();

    // Root: main area + action bar (3 rows)
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    // Main: task list (30%) + right panel (70%)
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(root[0]);

    render_task_list(f, state, main[0]);
    render_right_panel(f, state, main[1]);
    render_action_bar(f, state, root[1]);

    // Overlays (rendered last so they appear on top)
    if state.show_debug {
        render_debug_overlay(f, state, area);
        return; // debug overlay is full-screen, skip other overlays
    }
    if state.show_quit_modal {
        render_quit_modal(f, state, area);
    }
    if state.pending_form.is_some() {
        render_new_task_form(f, state, area);
    }
}

// ── Task List ─────────────────────────────────────────────────────────────

fn render_task_list(f: &mut Frame, state: &AppState, area: Rect) {
    let focused = state.focus == Focus::TaskList;
    let border_style = if focused {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Tasks ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let items: Vec<ListItem> = state
        .tasks
        .iter()
        .map(|t| {
            let icon = t.status.icon();
            let color = status_color(&t.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::raw(t.name.clone()),
            ]))
        })
        .collect();

    let mut list_state = ListState::default();
    if !state.tasks.is_empty() {
        list_state.select(Some(state.selected));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list, area, &mut list_state);
}

// ── Right Panel ───────────────────────────────────────────────────────────

fn render_right_panel(f: &mut Frame, state: &mut AppState, area: Rect) {
    let task = state.selected_task();
    let has_screenshot = task.map(|t| t.screenshot_path.is_some()).unwrap_or(false);

    let constraints = if has_screenshot {
        vec![Constraint::Percentage(65), Constraint::Percentage(35)]
    } else {
        vec![Constraint::Percentage(100)]
    };

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_chat_panel(f, state, split[0]);
    if has_screenshot {
        render_screenshot_panel(f, state, split[1]);
    }
}

fn render_chat_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let focused = state.focus == Focus::Chat;
    let border_style = if focused {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Header: task name + agent + mode + status
    let header = state.selected_task().map(|t| {
        format!(
            " {} [{}] [{}] [{}] ",
            t.name,
            t.agent.name,
            match t.mode {
                crate::task::ExecutionMode::Plan => "plan",
                crate::task::ExecutionMode::Direct => "direct",
            },
            t.status.icon()
        )
    }).unwrap_or_else(|| " No task selected ".to_string());

    let block = Block::default()
        .title(header)
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if state.tasks.is_empty() {
        let help = Paragraph::new("Press [n] to create a new task")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(help, inner);
        return;
    }

    // Inline input overlay at bottom when editing
    let chat_area = if state.inline_input.is_some() {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        let buf = state.inline_input.as_deref().unwrap_or("");
        let input_line = Paragraph::new(format!("> {}_", buf))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(input_line, split[1]);
        split[0]
    } else {
        inner
    };

    let messages = state.selected_messages();
    let widget = ChatWidget::new(messages, state.chat_scroll);
    f.render_widget(widget, chat_area);
}

fn render_screenshot_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" Screenshot ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Phase 3 will render the actual image via ratatui-image here.
    // For now, show the path as a placeholder.
    if let Some(path) = state.selected_task().and_then(|t| t.screenshot_path.as_ref()) {
        let text = Paragraph::new(format!("[image: {}]", path.display()))
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        f.render_widget(text, inner);
    }
}

// ── Action Bar ────────────────────────────────────────────────────────────

fn render_action_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let actions = build_action_hints(state);
    let line = Line::from(actions);
    f.render_widget(Paragraph::new(line), inner);
}

fn build_action_hints(state: &AppState) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let key = |k: &'static str| Span::styled(k, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    let label = |l: &'static str| Span::raw(format!(" {}  ", l));

    // Context-sensitive based on selected task status
    match state.selected_task_status() {
        Some(TaskStatus::WaitingApproval) => {
            spans.extend([key("[a]"), label("Accept"), key("[r]"), label("Reject"), key("[e]"), label("Edit prompt")]);
        }
        Some(TaskStatus::Running) | Some(TaskStatus::Executing) => {
            spans.extend([key("[^k]"), label("Kill task")]);
        }
        Some(TaskStatus::Success) | Some(TaskStatus::Failed(_)) | Some(TaskStatus::Cancelled) => {
            spans.extend([key("[d]"), label("Delete task")]);
        }
        _ => {}
    }

    // Global hints
    spans.extend([
        Span::raw("  "),
        key("[n]"), label("New task"),
        key("[Tab]"), label("Switch panel"),
        key("[?]"), label("Debug log"),
        key("[q]"), label("Quit"),
    ]);

    spans
}

// ── Quit Modal ────────────────────────────────────────────────────────────

fn render_quit_modal(f: &mut Frame, state: &AppState, area: Rect) {
    let active_count = state.tasks.iter().filter(|t| t.status.is_active()).count();
    let modal_text = format!(
        "{} task(s) still running.\nKill all and quit? [s/y = yes, n/Esc = cancel]",
        active_count
    );

    let modal_area = centered_rect(50, 20, area);
    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Quit ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let p = Paragraph::new(modal_text)
        .block(block)
        .wrap(Wrap { trim: true });

    f.render_widget(p, modal_area);
}

// ── New Task Form ─────────────────────────────────────────────────────────

fn render_new_task_form(f: &mut Frame, state: &AppState, area: Rect) {
    let form = match &state.pending_form {
        Some(f) => f,
        None => return,
    };

    let modal_area = centered_rect(60, 60, area);
    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" New Task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let agent_name = state
        .config
        .agents
        .get(form.agent_idx)
        .map(|a| a.name.as_str())
        .unwrap_or("(none)");

    let mode_str = match form.mode {
        crate::task::ExecutionMode::Plan => "plan",
        crate::task::ExecutionMode::Direct => "direct",
    };

    let field_style = |active: bool| {
        if active {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        }
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Name
            Constraint::Length(2), // Agent
            Constraint::Length(2), // Mode
            Constraint::Min(3),    // Prompt
            Constraint::Length(1), // Hint
        ])
        .split(inner);

    // Name
    f.render_widget(
        Paragraph::new(format!("Name: {}_", form.name))
            .style(field_style(form.active_field == FormField::Name)),
        rows[0],
    );
    // Agent
    f.render_widget(
        Paragraph::new(format!("Agent: {} (Enter/j/k to change)", agent_name))
            .style(field_style(form.active_field == FormField::Agent)),
        rows[1],
    );
    // Mode
    f.render_widget(
        Paragraph::new(format!("Mode: {} (Enter/space to toggle)", mode_str))
            .style(field_style(form.active_field == FormField::Mode)),
        rows[2],
    );
    // Prompt
    f.render_widget(
        Paragraph::new(format!("Prompt:\n{}_", form.prompt))
            .style(field_style(form.active_field == FormField::Prompt))
            .wrap(Wrap { trim: false }),
        rows[3],
    );
    // Hint
    f.render_widget(
        Paragraph::new("[Tab] next field  [Enter] submit/advance  [Esc] cancel")
            .style(Style::default().fg(Color::DarkGray)),
        rows[4],
    );
}

// ── Utilities ─────────────────────────────────────────────────────────────

// ── Debug Overlay ─────────────────────────────────────────────────────────

fn render_debug_overlay(f: &mut Frame, state: &AppState, area: Rect) {
    f.render_widget(Clear, area);

    let count = state.debug_logs.len();
    let title = format!(
        " Debug Log ({} entries) — [?/q/Esc] close  [j/k] scroll  [g/G] top/bottom  tail: .orchestrator/debug.log ",
        count
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width as usize;

    // Expand each log entry into wrapped display lines, keeping color per entry
    let mut display_lines: Vec<Line> = Vec::new();
    for msg in &state.debug_logs {
        let color = debug_line_color(msg);
        let style = Style::default().fg(color);
        // Word-wrap at inner width
        let mut remaining = msg.as_str();
        let mut first = true;
        while !remaining.is_empty() {
            let take = remaining
                .char_indices()
                .scan(0usize, |col, (i, c)| {
                    *col += 1;
                    if *col > width { None } else { Some(i) }
                })
                .last()
                .map(|i| {
                    // advance by one char
                    let mut end = i;
                    let c = remaining[i..].chars().next().unwrap();
                    end += c.len_utf8();
                    end
                })
                .unwrap_or(remaining.len());
            let prefix = if first { "" } else { "  " };
            display_lines.push(Line::from(Span::styled(
                format!("{}{}", prefix, &remaining[..take]),
                style,
            )));
            remaining = &remaining[take..];
            first = false;
        }
    }

    let total = display_lines.len();
    let visible = inner.height as usize;

    let start = if total > visible {
        let max_scroll = total - visible;
        let scroll = state.debug_scroll.min(max_scroll);
        total - visible - scroll
    } else {
        0
    };

    let page: Vec<Line> = display_lines.into_iter().skip(start).take(visible).collect();
    f.render_widget(Paragraph::new(page), inner);
}

fn debug_line_color(msg: &str) -> Color {
    if msg.contains("FAILED") || msg.contains("[stderr]") || msg.contains("error") {
        Color::Red
    } else if msg.contains("[status:") {
        Color::Yellow
    } else if msg.contains("[trigger]") || msg.contains("[plan]") {
        Color::Green
    } else if msg.contains("[spawn]") || msg.contains("[worktree]") {
        Color::Cyan
    } else if msg.contains("[log:") {
        Color::DarkGray
    } else {
        Color::White
    }
}

fn status_color(status: &TaskStatus) -> Color {
    match status {
        TaskStatus::Success => Color::Green,
        TaskStatus::Failed(_) => Color::Red,
        TaskStatus::WaitingApproval => Color::Yellow,
        TaskStatus::Running | TaskStatus::Executing => Color::Cyan,
        TaskStatus::Cancelled => Color::DarkGray,
        _ => Color::White,
    }
}

/// Returns a centered Rect with percentage width/height within `r`
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
