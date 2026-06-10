use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{chat::ChatMessage, config::AgentProfile};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Idle,
    CreatingWorktree,
    Running,
    /// Plan mode: waiting for user accept/reject
    WaitingApproval,
    /// Plan accepted, agent applying changes
    Executing,
    Success,
    Failed(String),
    Cancelled,
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            TaskStatus::CreatingWorktree
                | TaskStatus::Running
                | TaskStatus::WaitingApproval
                | TaskStatus::Executing
        )
    }

    pub fn icon(&self) -> &'static str {
        match self {
            TaskStatus::Idle => "○",
            TaskStatus::CreatingWorktree => "⚙",
            TaskStatus::Running => "▶",
            TaskStatus::WaitingApproval => "⏳",
            TaskStatus::Executing => "▶",
            TaskStatus::Success => "✓",
            TaskStatus::Failed(_) => "✗",
            TaskStatus::Cancelled => "○",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutionMode {
    Plan,
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub name: String,
    pub worktree_path: PathBuf,
    pub agent: AgentProfile,
    pub prompt: String,
    pub mode: ExecutionMode,
    pub status: TaskStatus,
    pub messages: Vec<ChatMessage>,
    pub screenshot_path: Option<PathBuf>,
}

impl Task {
    pub fn new(
        name: impl Into<String>,
        agent: AgentProfile,
        prompt: impl Into<String>,
        mode: ExecutionMode,
        worktree_base: &std::path::Path,
    ) -> Self {
        let id = Uuid::new_v4();
        let name = name.into();
        let short_id = &id.to_string()[..8];
        let worktree_path = worktree_base.join(format!("{}-{}", name, short_id));

        Self {
            id,
            name,
            worktree_path,
            agent,
            prompt: prompt.into(),
            mode,
            status: TaskStatus::Idle,
            messages: Vec::new(),
            screenshot_path: None,
        }
    }

    /// Add a system message (worktree created, errors, etc.)
    pub fn push_system(&mut self, msg: impl Into<String>) {
        self.messages.push(ChatMessage::system(msg));
    }

    /// Start a new streaming agent message, returning its index
    pub fn begin_agent_message(&mut self) -> usize {
        self.messages.push(ChatMessage::agent_start());
        self.messages.len() - 1
    }

    /// Append a line to the last agent message, or start one if none exists
    pub fn append_agent_line(&mut self, line: &str) {
        let needs_new = self
            .messages
            .last()
            .map(|m| m.role != crate::chat::MessageRole::Agent || !m.is_streaming)
            .unwrap_or(true);

        if needs_new {
            self.begin_agent_message();
        }

        if let Some(msg) = self.messages.last_mut() {
            msg.push_line(line);
        }
    }

    pub fn finish_agent_message(&mut self) {
        if let Some(msg) = self.messages.last_mut() {
            if msg.role == crate::chat::MessageRole::Agent {
                msg.finish_streaming();
            }
        }
    }
}
