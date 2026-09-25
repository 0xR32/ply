//! Panes: the registry of workspaces, tabs and panes ([`registry`]), each pane's task ([`pane`]) and spawning
//! ([`launch`]); the agent status machine joins them in WP6.

pub mod launch;
pub mod pane;
pub mod registry;
