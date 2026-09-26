//! The task queue's state (Ruling R60): every task plyd knows, each pane's queue order, and each queue's pause and block.
//!
//! This module holds no I/O. Each change returns the tasks it changed, which the registry stores and announces with
//! `task.changed`; a queue's pause or block comes back as the new [`QueueState`] for `queue.changed`. A queued task
//! belongs to one queue: its pane's, or, while no pane has taken it, the pool of its workspace, CLI and directory.
//! `position` numbers a queue's queued tasks from 0 without gaps, and a task that leaves `queued` keeps the position it
//! had. Finished tasks are kept as history, [`KEEP_FINISHED`] per workspace.

use std::collections::{BTreeMap, HashMap, HashSet};

use ply_proto::control::{ErrorCode, MAX_LINE_BYTES};
use ply_proto::pane::{
    AgentCli, BlockReason, MAX_TASK_TEXT_BYTES, PaneId, PauseReason, QueueState, Task, TaskId,
    TaskList, TaskState, UnixSeconds,
};

use crate::panes::registry::{MethodResult, refuse};

/// Finished tasks kept per workspace; older ones are deleted.
pub const KEEP_FINISHED: usize = 200;

/// Most bytes of tasks `task.list` answers with, leaving room in the C1 line for the response and the queues' flags.
pub const LIST_BYTES: usize = MAX_LINE_BYTES - 64 * 1024;

/// Detail of a task whose pane closed before or while it ran.
pub const PANE_CLOSED: &str = "the pane was closed";

/// Detail of a task that was running when plyd stopped.
pub const PLYD_RESTARTED: &str = "plyd restarted while it ran";

/// Detail of a task whose lost pane reopened as a login shell, which takes no tasks (Ruling R50).
pub const REOPENED_AS_SHELL: &str = "the pane reopened as a shell";

/// Detail of a pool task no pane had taken when plyd stopped.
pub const POOL_RESTARTED: &str = "plyd restarted before a pane took it";

/// Longest skill invocation `task.add` accepts, in bytes.
pub const MAX_SKILL_BYTES: usize = 256;

/// Refuses task text plyd will not type: empty, over [`MAX_TASK_TEXT_BYTES`], or holding a control character other
/// than newline and tab (a carriage return, ESC or NUL would act as keys, not text); errors: `bad_request`.
pub fn check_text(text: &str) -> MethodResult<()> {
    if text.trim().is_empty() {
        return Err(refuse(ErrorCode::BadRequest, "the task has no text"));
    }
    if text.len() > MAX_TASK_TEXT_BYTES {
        return Err(refuse(
            ErrorCode::BadRequest,
            format!(
                "the task is {} bytes; the limit is {MAX_TASK_TEXT_BYTES}",
                text.len()
            ),
        ));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(refuse(
            ErrorCode::BadRequest,
            "the task holds a control character; only newlines and tabs are typed",
        ));
    }
    Ok(())
}

/// The queue a queued task waits in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum QueueKey {
    /// A pane's own queue.
    Pane(PaneId),
    /// A workspace's pool for one CLI and directory.
    Pool {
        /// Workspace of the pool.
        workspace_id: u64,
        /// The CLI a pane must run to take from it.
        cli: AgentCli,
        /// The directory a pane must be in, or below.
        cwd: String,
    },
}

impl QueueKey {
    /// The queue `task` belongs to: its pane's when it has one, else its pool's; `None` for a task with neither.
    pub fn of(task: &Task) -> Option<Self> {
        match (task.pane_id, &task.pool) {
            (Some(pane), _) => Some(Self::Pane(pane)),
            (None, Some(pool)) => Some(Self::Pool {
                workspace_id: task.workspace_id,
                cli: pool.cli,
                cwd: pool.cwd.clone(),
            }),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Flags {
    paused: Option<PauseReason>,
    blocked: Option<BlockReason>,
}

/// Every task plyd knows and each pane's queue flags; see the module docs.
#[derive(Debug, Default)]
pub struct Queues {
    tasks: BTreeMap<TaskId, Task>,
    flags: HashMap<PaneId, Flags>,
}

fn is_finished(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Ended | TaskState::Failed | TaskState::Cancelled
    )
}

impl Queues {
    /// The queues from the stored tasks at plyd's start: in-flight tasks fail, queued tasks of `open` panes wait with
    /// their queue paused (`restored`), and every other queued task is cancelled. Returns the tasks it changed.
    pub fn restore(
        tasks: Vec<Task>,
        open: &HashSet<PaneId>,
        now: UnixSeconds,
    ) -> (Self, Vec<Task>) {
        let mut q = Self::default();
        let mut changed: BTreeMap<TaskId, Task> = BTreeMap::new();
        for mut task in tasks {
            match (task.state, task.pane_id) {
                (TaskState::Sent | TaskState::Running, _) => {
                    finish(
                        &mut task,
                        TaskState::Failed,
                        Some(PLYD_RESTARTED.to_owned()),
                        now,
                    );
                    changed.insert(task.id, task.clone());
                }
                (TaskState::Queued, Some(pane)) if open.contains(&pane) => {
                    q.flags.entry(pane).or_default().paused = Some(PauseReason::Restored);
                }
                (TaskState::Queued, Some(_)) => {
                    finish(
                        &mut task,
                        TaskState::Cancelled,
                        Some(PANE_CLOSED.to_owned()),
                        now,
                    );
                    changed.insert(task.id, task.clone());
                }
                (TaskState::Queued, None) => {
                    finish(
                        &mut task,
                        TaskState::Cancelled,
                        Some(POOL_RESTARTED.to_owned()),
                        now,
                    );
                    changed.insert(task.id, task.clone());
                }
                _ => {}
            }
            q.tasks.insert(task.id, task);
        }
        let keys: HashSet<QueueKey> = q
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Queued)
            .filter_map(QueueKey::of)
            .collect();
        for key in keys {
            for task in q.renumber(&key) {
                changed.insert(task.id, task);
            }
        }
        (q, changed.into_values().collect())
    }

    /// The task `id`.
    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// The tasks of `workspace_id` as `task.list` returns them, with the flags of those `panes` that are paused or blocked:
    /// every open task, then the finished ones newest first while the tasks fit in [`LIST_BYTES`] as JSON.
    pub fn list(&self, workspace_id: u64, panes: &HashSet<PaneId>) -> TaskList {
        let mut open: Vec<Task> = Vec::new();
        let mut finished: Vec<Task> = Vec::new();
        for task in self
            .tasks
            .values()
            .filter(|t| t.workspace_id == workspace_id)
        {
            if is_finished(task.state) {
                finished.push(task.clone());
            } else {
                open.push(task.clone());
            }
        }
        open.sort_by_key(|t| {
            (
                u8::from(t.state == TaskState::Queued),
                t.pane_id.unwrap_or(PaneId::MAX),
                t.position,
                t.id,
            )
        });
        finished.sort_by_key(|t| std::cmp::Reverse((t.ended_at, t.id)));
        let mut used = open
            .iter()
            .map(encoded_len)
            .fold(0usize, usize::saturating_add);
        for task in finished {
            used = used.saturating_add(encoded_len(&task));
            if used > LIST_BYTES {
                break;
            }
            open.push(task);
        }
        let mut queues: Vec<QueueState> = self
            .flags
            .iter()
            .filter(|(pane, f)| panes.contains(pane) && (f.paused.is_some() || f.blocked.is_some()))
            .map(|(pane, f)| QueueState {
                pane_id: *pane,
                paused: f.paused,
                blocked: f.blocked,
            })
            .collect();
        queues.sort_by_key(|q| q.pane_id);
        TaskList {
            tasks: open,
            queues,
        }
    }

    /// Bytes of text the queued, sent and running tasks of `workspace_id` hold together.
    pub fn open_text_bytes(&self, workspace_id: u64) -> usize {
        self.tasks
            .values()
            .filter(|t| t.workspace_id == workspace_id && !is_finished(t.state))
            .map(|t| t.text.len())
            .sum()
    }

    /// How many queued tasks `key` holds, which is also the next task's position.
    pub fn queued_len(&self, key: &QueueKey) -> usize {
        self.queued(key).len()
    }

    /// Adds a task that already has its id.
    pub fn insert(&mut self, task: Task) {
        self.tasks.insert(task.id, task);
    }

    /// Cancels a queued task with `detail`; `not_found` for an unknown task, `invalid_state` for one that is not queued.
    pub fn cancel(
        &mut self,
        id: TaskId,
        detail: &str,
        now: UnixSeconds,
    ) -> MethodResult<Vec<Task>> {
        self.require_queued(id)?;
        Ok(self.set_state(id, TaskState::Cancelled, Some(detail.to_owned()), now))
    }

    /// Moves a queued task to `position` in its queue (past the end: last); errors as [`Queues::cancel`].
    pub fn move_to(&mut self, id: TaskId, position: u32) -> MethodResult<Vec<Task>> {
        let key = self.require_queued(id)?;
        let mut order: Vec<TaskId> = self.queued(&key).iter().map(|t| t.id).collect();
        order.retain(|t| *t != id);
        let at = usize::try_from(position)
            .unwrap_or(usize::MAX)
            .min(order.len());
        order.insert(at, id);
        Ok(self.apply_order(&order))
    }

    /// Pauses (`Some`) or resumes (`None`) a pane's queue; the new state when it changed.
    pub fn pause(&mut self, pane: PaneId, reason: Option<PauseReason>) -> Option<QueueState> {
        let flags = self.flags.entry(pane).or_default();
        if flags.paused == reason {
            return None;
        }
        flags.paused = reason;
        Some(self.state_of(pane))
    }

    /// Blocks (`Some`) or unblocks (`None`) a pane's queue; the new state when it changed.
    pub fn block(&mut self, pane: PaneId, reason: Option<BlockReason>) -> Option<QueueState> {
        let flags = self.flags.entry(pane).or_default();
        if flags.blocked == reason {
            return None;
        }
        flags.blocked = reason;
        Some(self.state_of(pane))
    }

    /// A pane's queue flags.
    pub fn state_of(&self, pane: PaneId) -> QueueState {
        let flags = self.flags.get(&pane).copied().unwrap_or_default();
        QueueState {
            pane_id: pane,
            paused: flags.paused,
            blocked: flags.blocked,
        }
    }

    /// The task a pane's queue offers next: its first queued task, unless the queue is paused.
    pub fn head(&self, pane: PaneId) -> Option<&Task> {
        if self.state_of(pane).paused.is_some() {
            return None;
        }
        self.queued(&QueueKey::Pane(pane)).into_iter().next()
    }

    /// The pane's task that was typed and has not finished, if any.
    pub fn active(&self, pane: PaneId) -> Option<&Task> {
        self.tasks.values().find(|t| {
            t.pane_id == Some(pane) && matches!(t.state, TaskState::Sent | TaskState::Running)
        })
    }

    /// Moves task `id` to `state` with its time and `detail`; a task that leaves `queued` renumbers its queue.
    pub fn set_state(
        &mut self,
        id: TaskId,
        state: TaskState,
        detail: Option<String>,
        now: UnixSeconds,
    ) -> Vec<Task> {
        let Some(task) = self.tasks.get_mut(&id) else {
            return Vec::new();
        };
        let left = task.state == TaskState::Queued && state != TaskState::Queued;
        let key = QueueKey::of(task);
        finish(task, state, detail, now);
        let mut changed = vec![task.clone()];
        if left && let Some(key) = key {
            changed.extend(self.renumber(&key));
        }
        changed
    }

    /// A closed pane's tasks: queued ones are cancelled, a typed one fails; its flags are dropped.
    pub fn close_pane(&mut self, pane: PaneId, now: UnixSeconds) -> Vec<Task> {
        self.end_pane(pane, PANE_CLOSED, now)
    }

    /// The tasks of a pane that can no longer take any, ended as [`Queues::close_pane`] does with `detail`.
    pub fn end_pane(&mut self, pane: PaneId, detail: &str, now: UnixSeconds) -> Vec<Task> {
        self.flags.remove(&pane);
        let ids: Vec<(TaskId, TaskState)> = self
            .tasks
            .values()
            .filter(|t| t.pane_id == Some(pane) && !is_finished(t.state))
            .map(|t| (t.id, t.state))
            .collect();
        let mut changed = Vec::new();
        for (id, state) in ids {
            let to = if state == TaskState::Queued {
                TaskState::Cancelled
            } else {
                TaskState::Failed
            };
            if let Some(task) = self.tasks.get_mut(&id) {
                finish(task, to, Some(detail.to_owned()), now);
                changed.push(task.clone());
            }
        }
        changed
    }

    /// Moves the oldest pool task of `workspace_id` for `cli` whose directory is `cwd` or above it onto `pane`'s queue;
    /// returns the tasks that changed (none when no pool task fits).
    pub fn claim(
        &mut self,
        pane: PaneId,
        workspace_id: u64,
        cli: AgentCli,
        cwd: &str,
    ) -> Vec<Task> {
        let dir = std::path::Path::new(cwd);
        let candidate = self
            .tasks
            .values()
            .filter(|t| {
                t.state == TaskState::Queued
                    && t.pane_id.is_none()
                    && t.workspace_id == workspace_id
            })
            .filter(|t| {
                t.pool
                    .as_ref()
                    .is_some_and(|p| p.cli == cli && dir.starts_with(&p.cwd))
            })
            .min_by_key(|t| (t.position, t.id))
            .map(|t| t.id);
        let Some(id) = candidate else {
            return Vec::new();
        };
        let position = u32::try_from(self.queued_len(&QueueKey::Pane(pane))).unwrap_or(u32::MAX);
        let Some(task) = self.tasks.get_mut(&id) else {
            return Vec::new();
        };
        let pool = QueueKey::of(task);
        task.pane_id = Some(pane);
        task.position = position;
        let mut changed = vec![task.clone()];
        if let Some(pool) = pool {
            changed.extend(self.renumber(&pool));
        }
        changed
    }

    /// Removes the finished tasks of `workspace_id` beyond the newest [`KEEP_FINISHED`] and returns their ids.
    pub fn prune(&mut self, workspace_id: u64) -> Vec<TaskId> {
        let mut finished: Vec<(Option<UnixSeconds>, TaskId)> = self
            .tasks
            .values()
            .filter(|t| t.workspace_id == workspace_id && is_finished(t.state))
            .map(|t| (t.ended_at, t.id))
            .collect();
        if finished.len() <= KEEP_FINISHED {
            return Vec::new();
        }
        finished.sort();
        let drop = finished.len() - KEEP_FINISHED;
        let removed: Vec<TaskId> = finished.into_iter().take(drop).map(|(_, id)| id).collect();
        for id in &removed {
            self.tasks.remove(id);
        }
        removed
    }

    fn queued(&self, key: &QueueKey) -> Vec<&Task> {
        let mut out: Vec<&Task> = self
            .tasks
            .values()
            .filter(|t| t.state == TaskState::Queued && QueueKey::of(t).as_ref() == Some(key))
            .collect();
        out.sort_by_key(|t| (t.position, t.id));
        out
    }

    fn require_queued(&self, id: TaskId) -> MethodResult<QueueKey> {
        let task = self
            .tasks
            .get(&id)
            .ok_or_else(|| refuse(ErrorCode::NotFound, format!("no task {id}")))?;
        if task.state != TaskState::Queued {
            return Err(refuse(
                ErrorCode::InvalidState,
                "the task is no longer queued: it was typed, finished or cancelled",
            ));
        }
        QueueKey::of(task).ok_or_else(|| refuse(ErrorCode::Internal, "the task has no queue"))
    }

    /// Numbers `key`'s queued tasks 0, 1, 2… in their current order; returns the ones whose position changed.
    fn renumber(&mut self, key: &QueueKey) -> Vec<Task> {
        let order: Vec<TaskId> = self.queued(key).iter().map(|t| t.id).collect();
        self.apply_order(&order)
    }

    fn apply_order(&mut self, order: &[TaskId]) -> Vec<Task> {
        let mut changed = Vec::new();
        for (i, id) in order.iter().enumerate() {
            let position = u32::try_from(i).unwrap_or(u32::MAX);
            if let Some(task) = self.tasks.get_mut(id)
                && task.position != position
            {
                task.position = position;
                changed.push(task.clone());
            }
        }
        changed
    }
}

/// Bytes `task` takes in a JSON array; a task that cannot be encoded never fits.
fn encoded_len(task: &Task) -> usize {
    serde_json::to_vec(task).map_or(usize::MAX, |v| v.len() + 1)
}

/// Sets `task`'s state, the time that state stamps, and `detail`.
fn finish(task: &mut Task, state: TaskState, detail: Option<String>, now: UnixSeconds) {
    task.state = state;
    match state {
        TaskState::Queued => {}
        TaskState::Sent => task.sent_at = Some(now),
        TaskState::Running => task.started_at = Some(task.started_at.unwrap_or(now)),
        TaskState::Ended | TaskState::Failed | TaskState::Cancelled => task.ended_at = Some(now),
    }
    if detail.is_some() {
        task.detail = detail;
    }
}

#[cfg(test)]
mod tests {
    use ply_proto::pane::TaskPool;

    use super::*;

    fn queued(id: TaskId, pane: Option<PaneId>, position: u32) -> Task {
        Task {
            id,
            workspace_id: 1,
            pane_id: pane,
            pool: pane.is_none().then(|| TaskPool {
                cli: AgentCli::Claude,
                cwd: "/Users/example/project".into(),
            }),
            text: format!("task {id}"),
            skill: None,
            state: TaskState::Queued,
            position,
            detail: None,
            created_at: 10,
            sent_at: None,
            started_at: None,
            ended_at: None,
        }
    }

    fn with(tasks: Vec<Task>) -> Queues {
        let mut q = Queues::default();
        for t in tasks {
            q.insert(t);
        }
        q
    }

    fn positions(q: &Queues, pane: PaneId) -> Vec<(TaskId, u32)> {
        let panes = HashSet::from([pane]);
        q.list(1, &panes)
            .tasks
            .into_iter()
            .filter(|t| t.pane_id == Some(pane) && t.state == TaskState::Queued)
            .map(|t| (t.id, t.position))
            .collect()
    }

    #[test]
    fn a_queue_counts_its_queued_tasks_and_offers_the_first_unless_paused() {
        let mut q = with(vec![
            queued(1, Some(7), 0),
            queued(2, Some(7), 1),
            queued(3, None, 0),
        ]);
        assert_eq!(q.queued_len(&QueueKey::Pane(7)), 2);
        assert_eq!(q.queued_len(&QueueKey::of(&queued(9, None, 0)).unwrap()), 1);
        assert_eq!(q.head(7).map(|t| t.id), Some(1));
        let paused = q.pause(7, Some(PauseReason::User)).unwrap();
        assert_eq!(paused.paused, Some(PauseReason::User));
        assert_eq!(
            q.pause(7, Some(PauseReason::User)),
            None,
            "no change, no event"
        );
        assert_eq!(q.head(7), None);
        assert_eq!(q.pause(7, None).unwrap().paused, None);
        assert_eq!(q.head(7).map(|t| t.id), Some(1));
        assert_eq!(q.head(8), None);
    }

    #[test]
    fn cancelling_renumbers_the_queue_and_only_queued_tasks_can_be_cancelled() {
        let mut q = with(vec![
            queued(1, Some(7), 0),
            queued(2, Some(7), 1),
            queued(3, Some(7), 2),
        ]);
        let changed = q.cancel(1, "cancelled", 50).unwrap();
        let ids: Vec<(TaskId, TaskState, u32)> = changed
            .iter()
            .map(|t| (t.id, t.state, t.position))
            .collect();
        assert_eq!(
            ids,
            [
                (1, TaskState::Cancelled, 0),
                (2, TaskState::Queued, 0),
                (3, TaskState::Queued, 1)
            ]
        );
        assert_eq!(changed[0].ended_at, Some(50));
        assert_eq!(changed[0].detail.as_deref(), Some("cancelled"));
        assert_eq!(
            q.cancel(1, "again", 51).unwrap_err().code,
            ErrorCode::InvalidState
        );
        assert_eq!(q.cancel(99, "x", 51).unwrap_err().code, ErrorCode::NotFound);
        q.set_state(2, TaskState::Sent, None, 60);
        assert_eq!(
            q.cancel(2, "x", 61).unwrap_err().code,
            ErrorCode::InvalidState,
            "a typed task stays"
        );
    }

    #[test]
    fn moving_reorders_and_a_position_past_the_end_is_last() {
        let mut q = with(vec![
            queued(1, Some(7), 0),
            queued(2, Some(7), 1),
            queued(3, Some(7), 2),
        ]);
        let changed = q.move_to(3, 0).unwrap();
        assert_eq!(positions(&q, 7), [(3, 0), (1, 1), (2, 2)]);
        assert_eq!(changed.len(), 3);
        q.move_to(3, 99).unwrap();
        assert_eq!(positions(&q, 7), [(1, 0), (2, 1), (3, 2)]);
        assert!(
            q.move_to(1, 0).unwrap().is_empty(),
            "nothing moved, nothing announced"
        );
    }

    #[test]
    fn states_carry_their_times_and_leaving_queued_renumbers() {
        let mut q = with(vec![queued(1, Some(7), 0), queued(2, Some(7), 1)]);
        let changed = q.set_state(1, TaskState::Sent, None, 20);
        assert_eq!(changed[0].sent_at, Some(20));
        assert_eq!((changed[1].id, changed[1].position), (2, 0));
        assert_eq!(q.active(7).map(|t| t.id), Some(1));
        assert_eq!(q.head(7).map(|t| t.id), Some(2));
        let running = q.set_state(1, TaskState::Running, None, 21);
        assert_eq!(running[0].started_at, Some(21));
        let ended = q.set_state(1, TaskState::Ended, None, 30);
        assert_eq!(ended[0].ended_at, Some(30));
        assert_eq!(
            ended[0].position, 0,
            "a finished task keeps its last position"
        );
        assert_eq!(q.active(7), None);
        assert!(q.set_state(99, TaskState::Ended, None, 30).is_empty());
    }

    #[test]
    fn closing_a_pane_cancels_its_queue_and_fails_its_typed_task() {
        let mut q = with(vec![
            queued(1, Some(7), 0),
            queued(2, Some(7), 1),
            queued(3, Some(8), 0),
        ]);
        q.set_state(1, TaskState::Running, None, 20);
        q.pause(7, Some(PauseReason::User));
        let changed = q.close_pane(7, 40);
        let states: Vec<(TaskId, TaskState)> = changed.iter().map(|t| (t.id, t.state)).collect();
        assert_eq!(states, [(1, TaskState::Failed), (2, TaskState::Cancelled)]);
        assert!(
            changed
                .iter()
                .all(|t| t.detail.as_deref() == Some(PANE_CLOSED))
        );
        assert_eq!(q.state_of(7).paused, None);
        assert_eq!(q.head(8).map(|t| t.id), Some(3));
    }

    #[test]
    fn a_restart_fails_what_ran_holds_what_waits_and_cancels_the_rest() {
        let mut running = queued(1, Some(7), 0);
        running.state = TaskState::Running;
        let stored = vec![
            running,
            queued(2, Some(7), 3),
            queued(3, Some(7), 5),
            queued(4, Some(9), 0),
            queued(5, None, 0),
        ];
        let (q, changed) = Queues::restore(stored, &HashSet::from([7]), 100);
        let got: Vec<(TaskId, TaskState, u32, Option<&str>)> = changed
            .iter()
            .map(|t| (t.id, t.state, t.position, t.detail.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                (1, TaskState::Failed, 0, Some(PLYD_RESTARTED)),
                (2, TaskState::Queued, 0, None),
                (3, TaskState::Queued, 1, None),
                (4, TaskState::Cancelled, 0, Some(PANE_CLOSED)),
                (5, TaskState::Cancelled, 0, Some(POOL_RESTARTED)),
            ]
        );
        assert_eq!(q.state_of(7).paused, Some(PauseReason::Restored));
        assert_eq!(q.head(7), None, "held until the user resumes the queue");
        assert_eq!(q.state_of(9).paused, None);
    }

    #[test]
    fn the_list_shows_open_tasks_first_then_history_newest_first_and_the_flagged_queues() {
        let mut q = with(vec![
            queued(1, Some(7), 0),
            queued(2, Some(7), 1),
            queued(3, None, 0),
            queued(4, Some(8), 0),
        ]);
        q.set_state(4, TaskState::Running, None, 15);
        q.set_state(1, TaskState::Ended, None, 20);
        q.set_state(2, TaskState::Cancelled, None, 30);
        q.block(7, Some(BlockReason::Typing));
        q.pause(99, Some(PauseReason::User));
        let mut other = queued(5, Some(7), 0);
        other.workspace_id = 2;
        q.insert(other);
        let list = q.list(1, &HashSet::from([7, 8]));
        let ids: Vec<TaskId> = list.tasks.iter().map(|t| t.id).collect();
        assert_eq!(ids, [4, 3, 2, 1]);
        assert_eq!(
            list.queues,
            [QueueState {
                pane_id: 7,
                paused: None,
                blocked: Some(BlockReason::Typing)
            }]
        );
    }

    fn pooled(id: TaskId, cli: AgentCli, cwd: &str, position: u32) -> Task {
        Task {
            pool: Some(TaskPool {
                cli,
                cwd: cwd.to_owned(),
            }),
            ..queued(id, None, position)
        }
    }

    #[test]
    fn a_pane_claims_the_oldest_pool_task_of_its_cli_for_its_folder_or_above() {
        let mut q = with(vec![
            pooled(1, AgentCli::Codex, "/Users/example/project", 0),
            pooled(2, AgentCli::Claude, "/Users/example/project", 0),
            pooled(3, AgentCli::Claude, "/Users/example/project", 1),
            pooled(4, AgentCli::Claude, "/Users/example/other", 0),
        ]);
        assert!(
            q.claim(7, 1, AgentCli::Claude, "/Users/example/projectile")
                .is_empty(),
            "a sibling folder is not below"
        );
        assert!(
            q.claim(7, 2, AgentCli::Claude, "/Users/example/project")
                .is_empty(),
            "another workspace"
        );
        let changed = q.claim(
            7,
            1,
            AgentCli::Claude,
            "/Users/example/project/.claude/worktrees/x",
        );
        assert_eq!(changed[0].id, 2);
        assert_eq!((changed[0].pane_id, changed[0].position), (Some(7), 0));
        assert!(changed[0].pool.is_some(), "the pool it came from is kept");
        assert!(
            changed.iter().any(|t| t.id == 3 && t.position == 0),
            "the pool renumbers"
        );
        assert_eq!(q.head(7).map(|t| t.id), Some(2));
        let codex = q.claim(8, 1, AgentCli::Codex, "/Users/example/project");
        assert_eq!(codex[0].id, 1);
    }

    #[test]
    fn the_list_fits_one_c1_line_and_drops_the_oldest_history_first() {
        use ply_proto::control::{MAX_LINE_BYTES, Response, ServerMsg, encode_line};
        let mut q = Queues::default();
        let big = "x\n".repeat(MAX_TASK_TEXT_BYTES / 2);
        for i in 0..KEEP_FINISHED {
            let id = TaskId::try_from(i).unwrap() + 1;
            let mut t = queued(id, Some(7), 0);
            t.text = big.clone();
            q.insert(t);
            q.set_state(id, TaskState::Ended, None, 1000 + id);
        }
        let mut open = queued(9999, Some(7), 0);
        open.text = big;
        q.insert(open);
        let list = q.list(1, &HashSet::new());
        let res = Response {
            id: u64::MAX,
            outcome: Ok(serde_json::to_value(&list).unwrap()),
        };
        let line = encode_line(&ServerMsg::Res(res)).expect("one line");
        assert!(line.len() <= MAX_LINE_BYTES);
        assert_eq!(list.tasks[0].id, 9999, "open tasks are always listed");
        let ids: Vec<TaskId> = list.tasks[1..].iter().map(|t| t.id).collect();
        assert_eq!(
            ids[0],
            TaskId::try_from(KEEP_FINISHED).unwrap(),
            "newest first"
        );
        assert!(
            ids.windows(2).all(|w| w[0] == w[1] + 1),
            "a run of the newest, none skipped"
        );
    }

    #[test]
    fn history_keeps_the_newest_finished_tasks_per_workspace() {
        let mut q = Queues::default();
        let total = KEEP_FINISHED + 3;
        for i in 0..total {
            let id = TaskId::try_from(i).unwrap() + 1;
            q.insert(queued(id, Some(7), 0));
            q.set_state(id, TaskState::Ended, None, 1000 + id);
        }
        q.insert(queued(9999, Some(7), 0));
        let removed = q.prune(1);
        assert_eq!(removed, [1, 2, 3]);
        assert_eq!(q.list(1, &HashSet::new()).tasks.len(), KEEP_FINISHED + 1);
        assert!(q.get(9999).is_some(), "an open task is never pruned");
    }
}
