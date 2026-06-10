use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Local>,
    /// True while the agent is still producing output for this message
    pub is_streaming: bool,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: Local::now(),
            is_streaming: false,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            timestamp: Local::now(),
            is_streaming: false,
        }
    }

    pub fn agent_start() -> Self {
        Self {
            role: MessageRole::Agent,
            content: String::new(),
            timestamp: Local::now(),
            is_streaming: true,
        }
    }

    /// Append a line to this message's content
    pub fn push_line(&mut self, line: &str) {
        if !self.content.is_empty() {
            self.content.push('\n');
        }
        self.content.push_str(line);
    }

    pub fn finish_streaming(&mut self) {
        self.is_streaming = false;
    }
}
