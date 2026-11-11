//! Panes: the registry of workspaces, tabs and panes ([`registry`]), each pane's task ([`pane`]) with its agent
//! integration ([`agent`]), the spec 6.3 state machine ([`state`]) and the rest a reload waits for ([`rest`]), and
//! spawning and resuming ([`launch`]).

pub mod agent;
pub mod launch;
pub mod pane;
pub mod registry;
pub mod rest;
pub mod state;
