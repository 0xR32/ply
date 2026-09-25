//! The pane's single render state: the only consumer of the terminal's dirty flags (`render.zig` resets them on
//! update), so every client's dirty tracking is layered on top of it by [`super::Engine`]'s row generations.

use std::ffi::c_void;
use std::ptr;

use ghostty_sys as sys;
use ply_proto::data::{CellFlags, Cursor, CursorShape, Style};

use super::cells::{self, RawCell, RefusedReads};
use super::{RowSink, check};
use crate::error::Result;

/// Global dirty state after an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dirty {
    Clean,
    Partial,
    Full,
}

/// A render state with its row and cell iterators.
pub(super) struct RenderState {
    state: sys::GhosttyRenderState,
    rows: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
    graphemes: Vec<u32>,
}

impl RenderState {
    pub(super) fn new() -> Result<Self> {
        let mut this = Self {
            state: ptr::null_mut(),
            rows: ptr::null_mut(),
            cells: ptr::null_mut(),
            graphemes: Vec::with_capacity(8),
        };
        // SAFETY: each out pointer receives a new handle (null allocator = default); `Drop` frees any created.
        unsafe {
            check(
                "ghostty_render_state_new",
                sys::ghostty_render_state_new(ptr::null(), &raw mut this.state),
            )?;
            check(
                "ghostty_render_state_row_iterator_new",
                sys::ghostty_render_state_row_iterator_new(ptr::null(), &raw mut this.rows),
            )?;
            check(
                "ghostty_render_state_row_cells_new",
                sys::ghostty_render_state_row_cells_new(ptr::null(), &raw mut this.cells),
            )?;
        }
        Ok(this)
    }

    /// Copies `term`'s viewport, consuming its dirty flags, and reports the global dirty state.
    pub(super) fn update(&mut self, term: sys::GhosttyTerminal) -> Result<Dirty> {
        let mut dirty = sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        // SAFETY: both handles are live and the engine serializes access to `term`; `dirty` is an int.
        unsafe {
            check(
                "ghostty_render_state_update",
                sys::ghostty_render_state_update(self.state, term),
            )?;
            check(
                "ghostty_render_state_get(DIRTY)",
                sys::ghostty_render_state_get(
                    self.state,
                    sys::GHOSTTY_RENDER_STATE_DATA_DIRTY,
                    (&raw mut dirty).cast::<c_void>(),
                ),
            )?;
        }
        Ok(match dirty {
            sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE => Dirty::Clean,
            sys::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => Dirty::Partial,
            _ => Dirty::Full,
        })
    }

    /// Calls `mark` with each dirty row index of the last update.
    pub(super) fn dirty_rows(&mut self, mut mark: impl FnMut(u16)) -> Result<()> {
        self.rewind()?;
        let mut y = 0u16;
        // SAFETY: the iterator was just positioned on this render state; `y` is a u16.
        while unsafe { sys::ghostty_render_state_row_iterator_next_dirty(self.rows, &raw mut y) } {
            mark(y);
        }
        Ok(())
    }

    /// Clears the global and per-row dirty flags.
    pub(super) fn clean(&mut self) -> Result<()> {
        // SAFETY: the handle is live.
        check("ghostty_render_state_clean", unsafe {
            sys::ghostty_render_state_clean(self.state)
        })
    }

    /// `(cols, rows)` of the copied viewport.
    pub(super) fn size(&self) -> Result<(u16, u16)> {
        let (mut cols, mut rows) = (0u16, 0u16);
        // SAFETY: the handle is live; both out pointers are u16.
        unsafe {
            check(
                "ghostty_render_state_get(COLS)",
                sys::ghostty_render_state_get(
                    self.state,
                    sys::GHOSTTY_RENDER_STATE_DATA_COLS,
                    (&raw mut cols).cast(),
                ),
            )?;
            check(
                "ghostty_render_state_get(ROWS)",
                sys::ghostty_render_state_get(
                    self.state,
                    sys::GHOSTTY_RENDER_STATE_DATA_ROWS,
                    (&raw mut rows).cast(),
                ),
            )?;
        }
        Ok((cols, rows))
    }

    /// The cursor of the last update in C2 terms; hidden at (0, 0) when it lies outside the viewport.
    pub(super) fn cursor(&self) -> Result<Cursor> {
        let mut c = sys::GhosttyRenderStateCursor {
            size: size_of::<sys::GhosttyRenderStateCursor>(),
            ..Default::default()
        };
        // SAFETY: the handle is live; `c` is a sized cursor struct.
        unsafe {
            check(
                "ghostty_render_state_get(CURSOR)",
                sys::ghostty_render_state_get(
                    self.state,
                    sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                    (&raw mut c).cast(),
                ),
            )?;
        }
        let shape = match c.visual_style {
            0 => CursorShape::Bar,
            2 => CursorShape::Underline,
            3 => CursorShape::BlockHollow,
            _ => CursorShape::Block,
        };
        Ok(if c.viewport_has_value {
            Cursor {
                col: c.viewport_x,
                row: c.viewport_y,
                shape,
                visible: c.visible,
                blinking: c.blinking,
            }
        } else {
            Cursor {
                col: 0,
                row: 0,
                shape,
                visible: false,
                blinking: c.blinking,
            }
        })
    }

    /// Feeds every row `want` selects, top to bottom, cell by cell into `sink`.
    pub(super) fn read_rows<S: RowSink>(
        &mut self,
        pane_id: u64,
        want: impl Fn(u16) -> bool,
        sink: &mut S,
    ) -> Result<()> {
        self.rewind()?;
        let mut y = 0u16;
        // SAFETY: the iterator was just positioned on this live render state.
        while unsafe { sys::ghostty_render_state_row_iterator_next(self.rows) } {
            if want(y) {
                self.read_row(pane_id, y, sink)?;
            }
            y = y.saturating_add(1);
        }
        Ok(())
    }

    fn read_row<S: RowSink>(&mut self, pane_id: u64, y: u16, sink: &mut S) -> Result<()> {
        let mut header: sys::GhosttyRow = 0;
        let mut wrapped = false;
        let mut view = sys::GhosttyCellsView {
            ptr: ptr::null(),
            len: 0,
        };
        // SAFETY: the row iterator is positioned on row `y`; every out pointer has the type its data kind names.
        unsafe {
            check(
                "ghostty_render_state_row_get(RAW)",
                sys::ghostty_render_state_row_get(
                    self.rows,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                    (&raw mut header).cast(),
                ),
            )?;
            let code = sys::ghostty_row_get(
                header,
                sys::GHOSTTY_ROW_DATA_WRAP,
                (&raw mut wrapped).cast(),
            );
            if code != sys::GHOSTTY_SUCCESS {
                tracing::warn!(
                    pane_id,
                    row = y,
                    code,
                    "libghostty-vt could not read a row's wrap flag; treated as unwrapped"
                );
                wrapped = false;
            }
            check(
                "ghostty_render_state_row_get(CELLS_RAW)",
                sys::ghostty_render_state_row_get(
                    self.rows,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW,
                    (&raw mut view).cast(),
                ),
            )?;
            check(
                "ghostty_render_state_row_get(CELLS)",
                sys::ghostty_render_state_row_get(
                    self.rows,
                    sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&raw mut self.cells).cast(),
                ),
            )?;
        }
        let raws: &[sys::GhosttyCell] = if view.ptr.is_null() || view.len == 0 {
            &[]
        } else {
            // SAFETY: the view borrows `len` cells owned by the render state until its next update.
            unsafe { std::slice::from_raw_parts(view.ptr, view.len) }
        };
        sink.begin_row(i32::from(y), wrapped);
        let mut refused = RefusedReads::default();
        for (x, &raw) in raws.iter().enumerate() {
            let cell = match RawCell::decode(raw) {
                Ok(cell) => cell,
                Err(e) => {
                    refused.note(e);
                    sink.cell(0, &Style::default(), CellFlags::empty(), &[]);
                    continue;
                }
            };
            if cell.is_blank() {
                sink.cell(0, &Style::default(), CellFlags::empty(), &[]);
                continue;
            }
            let x = u16::try_from(x).unwrap_or(u16::MAX);
            let mut style = Style::default();
            if cell.styled || cell.has_graphemes() {
                // SAFETY: the cell iterator was positioned on this row above; `x` is within it.
                check("ghostty_render_state_row_cells_select", unsafe {
                    sys::ghostty_render_state_row_cells_select(self.cells, x)
                })?;
            }
            if cell.styled {
                let mut raw_style = cells::empty_style();
                // SAFETY: the selected cell's style is written into a sized GhosttyStyle.
                check("ghostty_render_state_row_cells_get(STYLE)", unsafe {
                    sys::ghostty_render_state_row_cells_get(
                        self.cells,
                        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                        (&raw mut raw_style).cast(),
                    )
                })?;
                style = cells::style(&raw_style);
            }
            match cell.tag_background(raw) {
                Ok(Some(bg)) => style.bg = bg,
                Ok(None) => {}
                Err(e) => refused.note(e),
            }
            let mut flags = cell.flags();
            let extra = if cell.has_graphemes() {
                self.read_graphemes()?
            } else {
                &[]
            };
            if !extra.is_empty() {
                flags = flags | CellFlags::GRAPHEME;
            }
            sink.cell(cell.codepoint, &style, flags, extra);
        }
        refused.log(pane_id, i64::from(y), "live");
        sink.end_row();
        Ok(())
    }

    /// The selected cell's codepoints after the base.
    fn read_graphemes(&mut self) -> Result<&[u32]> {
        let mut len = 0u32;
        // SAFETY: the cell iterator has a selected cell; `len` is a u32.
        check(
            "ghostty_render_state_row_cells_get(GRAPHEMES_LEN)",
            unsafe {
                sys::ghostty_render_state_row_cells_get(
                    self.cells,
                    sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    (&raw mut len).cast(),
                )
            },
        )?;
        self.graphemes.clear();
        self.graphemes.resize(len as usize, 0);
        if len > 1 {
            // SAFETY: the buffer holds `len` u32 slots, as GRAPHEMES_BUF requires.
            check(
                "ghostty_render_state_row_cells_get(GRAPHEMES_BUF)",
                unsafe {
                    sys::ghostty_render_state_row_cells_get(
                        self.cells,
                        sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                        self.graphemes.as_mut_ptr().cast(),
                    )
                },
            )?;
            Ok(&self.graphemes[1..])
        } else {
            Ok(&[])
        }
    }

    fn rewind(&mut self) -> Result<()> {
        // SAFETY: both handles are live; the iterator is re-positioned before the first row.
        check("ghostty_render_state_get(ROW_ITERATOR)", unsafe {
            sys::ghostty_render_state_get(
                self.state,
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&raw mut self.rows).cast(),
            )
        })
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: each handle is null or was created by this value and is freed once; the free functions accept null.
        unsafe {
            sys::ghostty_render_state_row_cells_free(self.cells);
            sys::ghostty_render_state_row_iterator_free(self.rows);
            sys::ghostty_render_state_free(self.state);
        }
    }
}
