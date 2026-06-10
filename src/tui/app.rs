use std::collections::HashMap;
use std::path::PathBuf;

use uuid::Uuid;

use crate::chat::ChatMessage;
use crate::config::{AgentProfile, Config};
use crate::messages::{AppEvent, TuiCommand, TuiCommandTx};
use crate::task::{ExecutionMode, Task, TaskStatus};

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    TaskList,
    Chat,
    NewTaskForm,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormField {
    Name,
    Agent,
    Mode,
    Prompt,
}

#[derive(Debug, Clone)]
pub struct NewTaskForm {
    pub name: String,
    pub agent_idx: usize,
    pub mode: ExecutionMode,
    pub prompt: String,
    pub active_field: FormField,
}

impl NewTaskForm {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            agent_idx: 0,
            mode: ExecutionMode::Plan,
            prompt: String::new(),
            active_field: FormField::Name,
        }
    }
}

pub struct AppState {
    pub tasks: Vec<Task>,
    pub selected: usize,
    /// Scroll offset in the chat panel (lines from bottom)
    pub chat_scroll: usize,
    pub focus: Focus,
    pub show_quit_modal: bool,
    pub pending_form: Option<NewTaskForm>,
    /// Inline prompt edit (when user presses [e] in WaitingApproval)
    pub inline_input: Option<String>,
    pub task_channels: HashMap<Uuid, TuiCommandTx>,
    pub config: Config,
    pub worktree_base: PathBuf,
}

impl AppState {
    pub fn new(config: Config, worktree_base: PathBuf) -> Self {
        Self {
            tasks: Vec::new(),
            selected: 0,
            chat_scroll: 0,
            focus: Focus::TaskList,
            show_quit_modal: false,
            pending_form: None,
            inline_input: None,
            task_channels: HashMap::new(),
            config,
            worktree_base,
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::LogLine { task_id, line } => {
                if let Some(task) = self.find_task_mut(task_id) {
                    task.append_agent_line(&line);
                    // Auto-scroll only if already at bottom
                    if self.chat_scroll == 0 {
                        self.chat_scroll = 0;
                    }
                }
            }
            AppEvent::StatusChange { task_id, status } => {
                if let Some(task) = self.find_task_mut(task_id) {
                    // Close streaming message when agent pauses/finishes
                    if matches!(
                        status,
                        TaskStatus::WaitingApproval
                            | TaskStatus::Success
                            | TaskStatus::Failed(_)
                            | TaskStatus::Cancelled
                    ) {
                        task.finish_agent_message();
                    }
                    task.status = status;
                }
            }
            AppEvent::Screenshot { task_id, path } => {
                if let Some(task) = self.find_task_mut(task_id) {
                    task.screenshot_path = Some(path);
                }
            }
            AppEvent::TaskDone { task_id } => {
                self.task_channels.remove(&task_id);
            }
        }
    }

    pub fn route_command(&mut self, cmd: TuiCommand) {
        if let Some(id) = self.selected_task_id() {
            if let Some(tx) = self.task_channels.get(&id) {
                let _ = tx.try_send(cmd);
            }
        }
    }

    pub fn kill_all_tasks(&mut self) {
        for tx in self.task_channels.values() {
            let _ = tx.try_send(TuiCommand::KillTask);
        }
    }

    pub fn has_active_tasks(&self) -> bool {
        self.tasks.iter().any(|t| t.status.is_active())
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.selected)
    }

    pub fn selected_task_id(&self) -> Option<Uuid> {
        self.tasks.get(self.selected).map(|t| t.id)
    }

    pub fn selected_task_status(&self) -> Option<&TaskStatus> {
        self.tasks.get(self.selected).map(|t| &t.status)
    }

    pub fn selected_messages(&self) -> &[ChatMessage] {
        self.tasks
            .get(self.selected)
            .map(|t| t.messages.as_slice())
            .unwrap_or(&[])
    }

    pub fn add_task(
        &mut self,
        name: String,
        agent: AgentProfile,
        prompt: String,
        mode: ExecutionMode,
    ) -> &Task {
        let task = Task::new(name, agent, prompt, mode, &self.worktree_base);
        self.tasks.push(task);
        self.selected = self.tasks.len() - 1;
        self.tasks.last().unwrap()
    }

    pub fn navigate_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.chat_scroll = 0;
        }
    }

    pub fn navigate_down(&mut self) {
        if !self.tasks.is_empty() && self.selected < self.tasks.len() - 1 {
            self.selected += 1;
            self.chat_scroll = 0;
        }
    }

    fn find_task_mut(&mut self, id: Uuid) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }
}
