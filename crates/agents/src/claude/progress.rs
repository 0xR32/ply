//! Claude Code plan progress from `PostToolUse` payloads (spec 6.4): TodoWrite lists and the TaskCreate/TaskUpdate tools (R16).

use ply_proto::pane::Progress;
use serde_json::Value;

use crate::adapter::str_field;
use crate::error::{Result, invalid};
use crate::plan::{ItemStatus, progress_of};

/// Tool whose `tool_input.todos` carries the whole list on every call.
pub const TODO_WRITE: &str = "TodoWrite";

/// Tool that adds one task; its id comes back in `tool_response`.
pub const TASK_CREATE: &str = "TaskCreate";

/// Tool that changes one task by `taskId`; status `deleted` removes it.
pub const TASK_UPDATE: &str = "TaskUpdate";

/// One item of the tracked list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Task id for the Task tools; `None` for TodoWrite items.
    pub id: Option<String>,
    /// Text shown as `Progress::current` while in progress.
    pub text: String,
    /// Current status.
    pub status: ItemStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Todos,
    Tasks,
}

/// Per-pane plan state; progress stays hidden until a TodoWrite or Task-tool call appears (R16), then the last-used source wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoState {
    todos: Vec<Item>,
    tasks: Vec<Item>,
    active: Option<Source>,
}

impl TodoState {
    /// Applies one PostToolUse call, returning whether the plan changed; a TodoWrite without `todos` fails and changes nothing.
    pub fn apply(
        &mut self,
        tool_name: &str,
        input: &Value,
        response: Option<&Value>,
    ) -> Result<bool> {
        let before = self.clone();
        match tool_name {
            TODO_WRITE => {
                self.todos = parse_todos(input)?;
                self.active = Some(Source::Todos);
            }
            TASK_CREATE => {
                let id = match created_task_id(response) {
                    Some(id) => id,
                    None => self.next_task_id(),
                };
                let text = task_text(input).unwrap_or(&id).to_owned();
                self.upsert_task(id, Some(text), ItemStatus::Pending);
                self.active = Some(Source::Tasks);
            }
            TASK_UPDATE => {
                let id = ["taskId", "task_id", "id"]
                    .iter()
                    .find_map(|k| id_field(input, k))
                    .ok_or_else(|| invalid("Claude TaskUpdate input", "no taskId"))?;
                match str_field(input, "status") {
                    Some("deleted") => self.tasks.retain(|t| t.id.as_deref() != Some(&id)),
                    status => {
                        let status = status.map(ItemStatus::parse);
                        let text = task_text(input).map(str::to_owned);
                        self.update_task(id, text, status);
                    }
                }
                self.active = Some(Source::Tasks);
            }
            _ => return Ok(false),
        }
        Ok(*self != before)
    }

    /// `done` = completed items, `total` = all items, `current` = the first in-progress item; `None` when hidden or empty.
    pub fn progress(&self) -> Option<Progress> {
        let items = match self.active? {
            Source::Todos => &self.todos,
            Source::Tasks => &self.tasks,
        };
        progress_of(items.iter().map(|i| (i.status, i.text.as_str())))
    }

    fn next_task_id(&self) -> String {
        let max = self
            .tasks
            .iter()
            .filter_map(|t| t.id.as_deref()?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        (max + 1).to_string()
    }

    fn upsert_task(&mut self, id: String, text: Option<String>, status: ItemStatus) {
        self.update_task(id, text, Some(status));
    }

    fn update_task(&mut self, id: String, text: Option<String>, status: Option<ItemStatus>) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id.as_deref() == Some(&id)) {
            if let Some(text) = text {
                task.text = text;
            }
            if let Some(status) = status {
                task.status = status;
            }
            return;
        }
        let text = match text {
            Some(text) => text,
            None => id.clone(),
        };
        self.tasks.push(Item {
            text,
            id: Some(id),
            status: status.unwrap_or(ItemStatus::Pending),
        });
    }
}

fn parse_todos(input: &Value) -> Result<Vec<Item>> {
    let todos = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Claude TodoWrite input", "no todos array"))?;
    Ok(todos
        .iter()
        .map(|todo| Item {
            id: None,
            text: ["content", "activeForm", "subject"]
                .iter()
                .find_map(|k| str_field(todo, k))
                .unwrap_or_default()
                .to_owned(),
            status: ItemStatus::parse(str_field(todo, "status").unwrap_or_default()),
        })
        .collect())
}

fn task_text(input: &Value) -> Option<&str> {
    ["subject", "title", "content", "description"]
        .iter()
        .find_map(|k| str_field(input, k).filter(|s| !s.is_empty()))
}

fn id_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn created_task_id(response: Option<&Value>) -> Option<String> {
    let response = response?;
    if let Some(id) = response.get("task").and_then(|t| id_field(t, "id")) {
        return Some(id);
    }
    if let Some(id) = ["taskId", "task_id", "id"]
        .iter()
        .find_map(|k| id_field(response, k))
    {
        return Some(id);
    }
    let text = response.as_str()?;
    let digits: String = text
        .split_once('#')?
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then_some(digits)
}
