use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};

use crate::chat::{ChatMessage, MessageRole};

/// Renders a list of ChatMessages as bordered bubbles in a given area.
/// `scroll` is lines from the bottom (0 = show most recent).
pub struct ChatWidget<'a> {
    pub messages: &'a [ChatMessage],
    pub scroll: usize,
}

impl<'a> ChatWidget<'a> {
    pub fn new(messages: &'a [ChatMessage], scroll: usize) -> Self {
        Self { messages, scroll }
    }
}

impl<'a> Widget for ChatWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Build all lines for all messages
        let mut all_lines: Vec<Line<'static>> = Vec::new();

        for msg in self.messages {
            let bubble_lines = render_bubble(msg, area.width as usize);
            all_lines.extend(bubble_lines);
            // Blank separator between messages
            all_lines.push(Line::from(""));
        }

        let total = all_lines.len();
        let visible = area.height as usize;

        let start = if total > visible {
            let max_scroll = total - visible;
            let scroll = self.scroll.min(max_scroll);
            total - visible - scroll
        } else {
            0
        };

        let text = Text::from(
            all_lines
                .into_iter()
                .skip(start)
                .take(visible)
                .collect::<Vec<_>>(),
        );

        Paragraph::new(text).render(area, buf);
    }
}

fn render_bubble(msg: &ChatMessage, width: usize) -> Vec<Line<'static>> {
    let (label, border_color, label_color) = match msg.role {
        MessageRole::User => ("user", Color::Blue, Color::Blue),
        MessageRole::Agent => ("agent", Color::Green, Color::Green),
        MessageRole::System => ("system", Color::DarkGray, Color::DarkGray),
    };

    let inner_width = width.saturating_sub(4); // "║ " + " ║"
    let border_width = width.saturating_sub(2);

    // Top border: ╔══ label ═══...╗
    let label_part = format!(" {} ", label);
    let fill_len = border_width.saturating_sub(label_part.len() + 1);
    let top = format!("╔══{}{}╗", label_part, "═".repeat(fill_len));
    let bottom = format!("╚{}╝", "═".repeat(border_width));

    let border_style = Style::default().fg(border_color);
    let _label_style = Style::default()
        .fg(label_color)
        .add_modifier(Modifier::BOLD);
    let streaming_suffix = if msg.is_streaming { "  ▌" } else { "" };

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Top border with colored label
    lines.push(Line::from(vec![
        Span::styled(top, border_style),
    ]));

    // Content lines
    let content = msg.content.clone() + streaming_suffix;
    for raw_line in content.lines() {
        // Wrap long lines manually
        let chunks = wrap_text(raw_line, inner_width);
        for chunk in chunks {
            lines.push(Line::from(vec![
                Span::styled("║ ", border_style),
                Span::raw(chunk),
            ]));
        }
    }
    // Empty content fallback
    if msg.content.is_empty() && msg.is_streaming {
        lines.push(Line::from(vec![
            Span::styled("║ ", border_style),
            Span::raw("▌"),
        ]));
    }

    lines.push(Line::from(vec![Span::styled(bottom, border_style)]));

    lines
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut result = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if current.len() >= width {
            result.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() || result.is_empty() {
        // Pad with spaces to maintain consistent width
        result.push(current);
    }
    result
}
