//! Panes: the registry of workspaces, tabs and panes ([`registry`]), each pane's task ([`pane`]) with its agent
//! integration ([`agent`]) and the spec 6.3 state machine ([`state`]), and spawning and resuming ([`launch`]).

pub mod agent;
pub mod dispatch;
pub mod launch;
pub mod pane;
pub mod queue;
pub mod registry;
pub mod state;
