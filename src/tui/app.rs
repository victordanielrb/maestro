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
    /// Debug/diagnostic log visible via [?]
    pub debug_logs: Vec<String>,
    pub show_debug: bool,
    pub debug_scroll: usize,
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
            debug_logs: Vec::new(),
            show_debug: false,
            debug_scroll: 0,
        }
    }

    pub fn apply_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::LogLine { task_id, line } => {
                let task_name = self.tasks.iter().find(|t| t.id == task_id)
                    .map(|t| t.name.clone()).unwrap_or_default();
                self.push_debug(format!("[log:{}] {}", task_name, &line[..line.len().min(120)]));
                if let Some(task) = self.find_task_mut(task_id) {
                    task.append_agent_line(&line);
                }
            }
            AppEvent::StatusChange { task_id, status } => {
                let task_name = self.tasks.iter().find(|t| t.id == task_id)
                    .map(|t| t.name.clone()).unwrap_or_default();
                self.push_debug(format!("[status:{}] {:?}", task_name, status));
                if let Some(task) = self.find_task_mut(task_id) {
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
                self.push_debug(format!("[screenshot] {:?}", path));
                if let Some(task) = self.find_task_mut(task_id) {
                    task.screenshot_path = Some(path);
                }
            }
            AppEvent::TaskDone { task_id } => {
                self.push_debug(format!("[done] task_id={}", task_id));
                self.task_channels.remove(&task_id);
            }
            AppEvent::Debug(msg) => {
                self.push_debug(msg);
            }
        }
    }

    fn push_debug(&mut self, msg: String) {
        self.debug_logs.push(msg);
        // Keep last 2000 lines to avoid unbounded growth
        if self.debug_logs.len() > 2000 {
            self.debug_logs.drain(..200);
        }
        // Auto-scroll to bottom when not manually scrolled
        if self.debug_scroll == 0 {
            self.debug_scroll = 0;
        }
    }

    pub fn toggle_debug(&mut self) {
        self.show_debug = !self.show_debug;
        self.debug_scroll = 0; // jump to bottom on open
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
