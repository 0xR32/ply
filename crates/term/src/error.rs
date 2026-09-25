/// Every way a ply-term call can fail; none panics, and a call that returns one leaves its receiver usable.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A libghostty-vt call returned a negative `GhosttyResult` (`types.h`: -1 out of memory, -2 invalid value, -3 out of space, -4 no value, -5 I/O, -6 limit, -7 rejected).
    #[error("libghostty-vt {call} failed with result {code}")]
    Ghostty {
        /// The C function that failed.
        call: &'static str,
        /// Its `GhosttyResult`.
        code: i32,
    },
    /// A grid of zero columns or rows was requested.
    #[error("invalid grid size {cols}x{rows}: both must be at least 1")]
    InvalidSize {
        /// Requested columns.
        cols: u16,
        /// Requested rows.
        rows: u16,
    },
    /// A Delta or History reached a [`crate::Replica`] before any Snapshot.
    #[error("frame kind {kind:#04x} arrived before the first Snapshot")]
    NoSnapshot {
        /// Kind byte of the frame.
        kind: u8,
    },
    /// A frame's sequence number did not increase.
    #[error("sequence number {got} does not follow {last}")]
    SequenceRegressed {
        /// Last sequence number applied.
        last: u64,
        /// The offending one.
        got: u64,
    },
    /// A cell referred to a style id the client's table does not hold.
    #[error("style id {id} is not in the style table")]
    UnknownStyle {
        /// The unknown id.
        id: u16,
    },
    /// `styles_added` re-used an id already in the table (ids never repeat within one attachment).
    #[error("style id {id} was added twice")]
    DuplicateStyle {
        /// The repeated id.
        id: u16,
    },
    /// A screen row index lay outside `0..rows`.
    #[error("row index {index} is outside the {rows}-row screen")]
    RowOutOfRange {
        /// The row index from the frame.
        index: i32,
        /// Rows of the screen.
        rows: u16,
    },
    /// A row carried more cells than the grid has columns.
    #[error("row {index} has {cells} cells, more than the {cols} columns")]
    RowTooWide {
        /// The row index from the frame.
        index: i32,
        /// Cells in the row.
        cells: usize,
        /// Columns of the grid.
        cols: u16,
    },
    /// A frame kind the Replica does not apply (it applies SNAPSHOT, DELTA and HISTORY).
    #[error("frame kind {kind:#04x} carries no screen content")]
    NotScreenFrame {
        /// Kind byte of the frame.
        kind: u8,
    },
}

/// `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
