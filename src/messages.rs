use std::path::PathBuf;

use uuid::Uuid;

use crate::task::TaskStatus;

// AppEvent: many async senders (controllers) → one sync receiver (TUI main thread)
// std::sync::mpsc because the receiver is on the synchronous main thread and
// uses try_recv() (non-blocking) before each ratatui frame.
pub type AppEventTx = std::sync::mpsc::Sender<AppEvent>;
pub type AppEventRx = std::sync::mpsc::Receiver<AppEvent>;

// TuiCommand: one sync sender per task (TUI) → one async receiver (controller)
// tokio::sync::mpsc because controllers await cmd_rx.recv() in WaitingApproval.
pub type TuiCommandTx = tokio::sync::mpsc::Sender<TuiCommand>;
pub type TuiCommandRx = tokio::sync::mpsc::Receiver<TuiCommand>;

pub const TUI_COMMAND_BUFFER: usize = 16;

#[derive(Debug)]
pub enum TuiCommand {
    AcceptPlan,
    RejectPlan,
    UpdatePrompt(String),
    KillTask,
}

#[derive(Debug)]
pub enum AppEvent {
    LogLine { task_id: Uuid, line: String },
    StatusChange { task_id: Uuid, status: TaskStatus },
    Screenshot { task_id: Uuid, path: PathBuf },
    TaskDone { task_id: Uuid },
}
