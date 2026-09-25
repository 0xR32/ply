//! C8: classification of Codex's OSC 9 notification bodies, per ADR-0004's table and Ruling R27.

/// Fixed prefixes that mean a dialog waits for approval (`ExecApprovalRequested`, `EditApprovalRequested`, `ElicitationRequested`).
pub const APPROVAL_PREFIXES: [&str; 3] = [
    "Approval requested: ",
    "Codex wants to edit ",
    "Approval requested by ",
];

/// Fixed prefix of `PlanModePrompt`, also used for tool-driven multi-question prompts.
pub const PLAN_PROMPT_PREFIX: &str = "Plan mode prompt: ";

/// Fixed prefix of `AsyncQuestion`.
pub const QUESTION_PREFIX: &str = "Question: ";

/// What one OSC 9 body means for the pane (spec 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc9Kind {
    /// An approval dialog is open → `waiting_permission`.
    Approval,
    /// A plan-mode or tool question prompt is open → `waiting_input`.
    PlanPrompt,
    /// An asynchronous question → `waiting_input`.
    Question,
    /// Any other body: the assistant's final text (or `Agent turn complete`) → `idle` (R27).
    TurnComplete,
}

/// Classifies one body; the known prefixes do not overlap, and everything else is [`Osc9Kind::TurnComplete`], never an error.
pub fn classify_osc9(body: &str) -> Osc9Kind {
    if APPROVAL_PREFIXES.iter().any(|p| body.starts_with(p)) {
        Osc9Kind::Approval
    } else if body.starts_with(PLAN_PROMPT_PREFIX) {
        Osc9Kind::PlanPrompt
    } else if body.starts_with(QUESTION_PREFIX) {
        Osc9Kind::Question
    } else {
        Osc9Kind::TurnComplete
    }
}
