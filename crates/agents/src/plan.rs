//! Plan items shared by both CLIs' progress sources (spec 6.4).

use ply_proto::pane::Progress;

/// Status of one plan item; unknown strings count as [`ItemStatus::Pending`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    /// Not started.
    Pending,
    /// Being worked on; its text becomes `Progress::current`.
    InProgress,
    /// Done.
    Completed,
}

impl ItemStatus {
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            _ => Self::Pending,
        }
    }
}

/// `(status, text)` pairs to [`Progress`] (`done` = completed, `current` = first in progress); `None` for an empty list.
pub(crate) fn progress_of<'a>(
    items: impl Iterator<Item = (ItemStatus, &'a str)>,
) -> Option<Progress> {
    let (mut done, mut total, mut current) = (0u32, 0u32, None);
    for (status, text) in items {
        total = total.saturating_add(1);
        match status {
            ItemStatus::Completed => done = done.saturating_add(1),
            ItemStatus::InProgress if current.is_none() => current = Some(text.to_owned()),
            _ => {}
        }
    }
    (total > 0).then_some(Progress {
        done,
        total,
        current,
    })
}
