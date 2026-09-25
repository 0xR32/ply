//! The hand-written declarations match the library's own ABI manifest (`ghostty_type_json()`, `types.h`):
//! every struct's size and field offsets and every enum constant ply declares. Rerun on every pin upgrade
//! (ADR-0008 Decision 5).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::CStr;
use std::mem::{offset_of, size_of};

use ghostty_sys::*;
use serde_json::Value;

fn manifest() -> Value {
    // SAFETY: ghostty_type_json returns a NUL-terminated string in static storage.
    let text = unsafe { CStr::from_ptr(ghostty_type_json()) };
    serde_json::from_str(text.to_str().expect("manifest is UTF-8")).expect("manifest is JSON")
}

fn layout(m: &Value, ty: &str, size: usize, fields: &[(&str, usize)]) {
    let t = &m["types"][ty];
    assert_eq!(t["size"].as_u64(), Some(size as u64), "{ty} size");
    for (field, offset) in fields {
        assert_eq!(
            t["fields"][*field]["offset"].as_u64(),
            Some(*offset as u64),
            "{ty}.{field} offset"
        );
    }
}

fn value(m: &Value, ty: &str, name: &str, got: i64) {
    assert_eq!(
        m["types"][ty]["values"][name].as_i64(),
        Some(got),
        "{ty}.{name}"
    );
}

#[test]
fn struct_layouts_match_the_manifest() {
    let m = manifest();
    assert_eq!(m["schema"], 1);
    layout(
        &m,
        "GhosttyString",
        size_of::<GhosttyString>(),
        &[("ptr", 0), ("len", 8)],
    );
    layout(
        &m,
        "GhosttyColorRgb",
        size_of::<GhosttyColorRgb>(),
        &[("r", 0), ("g", 1), ("b", 2)],
    );
    layout(
        &m,
        "GhosttyStyleColor",
        size_of::<GhosttyStyleColor>(),
        &[
            ("tag", offset_of!(GhosttyStyleColor, tag)),
            ("value", offset_of!(GhosttyStyleColor, value)),
        ],
    );
    layout(
        &m,
        "GhosttyStyleColorValue",
        size_of::<GhosttyStyleColorValue>(),
        &[],
    );
    layout(
        &m,
        "GhosttyStyle",
        size_of::<GhosttyStyle>(),
        &[
            ("size", offset_of!(GhosttyStyle, size)),
            ("fg_color", offset_of!(GhosttyStyle, fg_color)),
            ("bg_color", offset_of!(GhosttyStyle, bg_color)),
            ("underline_color", offset_of!(GhosttyStyle, underline_color)),
            ("bold", offset_of!(GhosttyStyle, bold)),
            ("italic", offset_of!(GhosttyStyle, italic)),
            ("faint", offset_of!(GhosttyStyle, faint)),
            ("blink", offset_of!(GhosttyStyle, blink)),
            ("inverse", offset_of!(GhosttyStyle, inverse)),
            ("invisible", offset_of!(GhosttyStyle, invisible)),
            ("strikethrough", offset_of!(GhosttyStyle, strikethrough)),
            ("overline", offset_of!(GhosttyStyle, overline)),
            ("underline", offset_of!(GhosttyStyle, underline)),
        ],
    );
    layout(
        &m,
        "GhosttyTerminalModeConfig",
        size_of::<GhosttyTerminalModeConfig>(),
        &[
            ("mode", 0),
            ("value", offset_of!(GhosttyTerminalModeConfig, value)),
        ],
    );
    layout(
        &m,
        "GhosttySizeReportSize",
        size_of::<GhosttySizeReportSize>(),
        &[
            ("rows", offset_of!(GhosttySizeReportSize, rows)),
            ("columns", offset_of!(GhosttySizeReportSize, columns)),
            ("cell_width", offset_of!(GhosttySizeReportSize, cell_width)),
            (
                "cell_height",
                offset_of!(GhosttySizeReportSize, cell_height),
            ),
        ],
    );
    layout(
        &m,
        "GhosttyTerminalDesktopNotification",
        size_of::<GhosttyTerminalDesktopNotification>(),
        &[
            (
                "title",
                offset_of!(GhosttyTerminalDesktopNotification, title),
            ),
            ("body", offset_of!(GhosttyTerminalDesktopNotification, body)),
        ],
    );
    layout(
        &m,
        "GhosttyTerminalProgressReport",
        size_of::<GhosttyTerminalProgressReport>(),
        &[
            ("state", offset_of!(GhosttyTerminalProgressReport, state)),
            (
                "progress",
                offset_of!(GhosttyTerminalProgressReport, progress),
            ),
        ],
    );
    layout(
        &m,
        "GhosttyClipboardContent",
        size_of::<GhosttyClipboardContent>(),
        &[
            ("mime", 0),
            ("data", offset_of!(GhosttyClipboardContent, data)),
        ],
    );
    layout(
        &m,
        "GhosttyClipboardWriteReply",
        size_of::<GhosttyClipboardWriteReply>(),
        &[
            ("result", offset_of!(GhosttyClipboardWriteReply, result)),
            ("remember", offset_of!(GhosttyClipboardWriteReply, remember)),
        ],
    );
    layout(
        &m,
        "GhosttyClipboardWrite",
        size_of::<GhosttyClipboardWrite>(),
        &[
            ("location", offset_of!(GhosttyClipboardWrite, location)),
            ("contents", offset_of!(GhosttyClipboardWrite, contents)),
            (
                "contents_len",
                offset_of!(GhosttyClipboardWrite, contents_len),
            ),
            ("name", offset_of!(GhosttyClipboardWrite, name)),
            ("granted", offset_of!(GhosttyClipboardWrite, granted)),
            (
                "can_remember",
                offset_of!(GhosttyClipboardWrite, can_remember),
            ),
            ("ctx", offset_of!(GhosttyClipboardWrite, ctx)),
            ("reply", offset_of!(GhosttyClipboardWrite, reply)),
        ],
    );
    layout(
        &m,
        "GhosttyRenderStateCursor",
        size_of::<GhosttyRenderStateCursor>(),
        &[
            (
                "viewport_has_value",
                offset_of!(GhosttyRenderStateCursor, viewport_has_value),
            ),
            (
                "viewport_x",
                offset_of!(GhosttyRenderStateCursor, viewport_x),
            ),
            (
                "viewport_y",
                offset_of!(GhosttyRenderStateCursor, viewport_y),
            ),
            ("wide_tail", offset_of!(GhosttyRenderStateCursor, wide_tail)),
            ("visible", offset_of!(GhosttyRenderStateCursor, visible)),
            ("blinking", offset_of!(GhosttyRenderStateCursor, blinking)),
            (
                "password_input",
                offset_of!(GhosttyRenderStateCursor, password_input),
            ),
            (
                "visual_style",
                offset_of!(GhosttyRenderStateCursor, visual_style),
            ),
        ],
    );
    layout(
        &m,
        "GhosttyCellsView",
        size_of::<GhosttyCellsView>(),
        &[("ptr", 0), ("len", 8)],
    );
    layout(
        &m,
        "GhosttyPointCoordinate",
        size_of::<GhosttyPointCoordinate>(),
        &[("x", 0), ("y", offset_of!(GhosttyPointCoordinate, y))],
    );
    layout(&m, "GhosttyPointValue", size_of::<GhosttyPointValue>(), &[]);
    layout(
        &m,
        "GhosttyPoint",
        size_of::<GhosttyPoint>(),
        &[("tag", 0), ("value", offset_of!(GhosttyPoint, value))],
    );
    layout(
        &m,
        "GhosttyGridRef",
        size_of::<GhosttyGridRef>(),
        &[
            ("node", offset_of!(GhosttyGridRef, node)),
            ("x", offset_of!(GhosttyGridRef, x)),
            ("y", offset_of!(GhosttyGridRef, y)),
        ],
    );
    layout(
        &m,
        "GhosttySelection",
        size_of::<GhosttySelection>(),
        &[
            ("start", offset_of!(GhosttySelection, start)),
            ("end", offset_of!(GhosttySelection, end)),
            ("rectangle", offset_of!(GhosttySelection, rectangle)),
        ],
    );
    layout(
        &m,
        "GhosttySelectionBuffer",
        size_of::<GhosttySelectionBuffer>(),
        &[("ptr", 0), ("cap", 8), ("len", 16)],
    );
    layout(
        &m,
        "GhosttyMousePosition",
        size_of::<GhosttyMousePosition>(),
        &[("x", 0), ("y", 4)],
    );
    layout(
        &m,
        "GhosttyMouseEncoderSize",
        size_of::<GhosttyMouseEncoderSize>(),
        &[
            (
                "screen_width",
                offset_of!(GhosttyMouseEncoderSize, screen_width),
            ),
            (
                "cell_height",
                offset_of!(GhosttyMouseEncoderSize, cell_height),
            ),
            (
                "padding_left",
                offset_of!(GhosttyMouseEncoderSize, padding_left),
            ),
        ],
    );
    layout(
        &m,
        "GhosttyWriter",
        size_of::<GhosttyWriter>(),
        &[("write", 0), ("userdata", 8)],
    );
    layout(
        &m,
        "GhosttyMimeReader",
        size_of::<GhosttyMimeReader>(),
        &[("read", 0), ("userdata", 8)],
    );
    layout(
        &m,
        "GhosttyPaste",
        size_of::<GhosttyPaste>(),
        &[
            ("location", offset_of!(GhosttyPaste, location)),
            ("source", offset_of!(GhosttyPaste, source)),
            ("mimes", offset_of!(GhosttyPaste, mimes)),
            ("mimes_len", offset_of!(GhosttyPaste, mimes_len)),
            ("reader", offset_of!(GhosttyPaste, reader)),
            ("allow_unsafe", offset_of!(GhosttyPaste, allow_unsafe)),
        ],
    );
    layout(
        &m,
        "GhosttyFormatterScreenExtra",
        size_of::<GhosttyFormatterScreenExtra>(),
        &[
            ("cursor", offset_of!(GhosttyFormatterScreenExtra, cursor)),
            (
                "charsets",
                offset_of!(GhosttyFormatterScreenExtra, charsets),
            ),
        ],
    );
    layout(
        &m,
        "GhosttyFormatterTerminalExtra",
        size_of::<GhosttyFormatterTerminalExtra>(),
        &[
            (
                "palette",
                offset_of!(GhosttyFormatterTerminalExtra, palette),
            ),
            (
                "keyboard",
                offset_of!(GhosttyFormatterTerminalExtra, keyboard),
            ),
            ("screen", offset_of!(GhosttyFormatterTerminalExtra, screen)),
        ],
    );
    layout(
        &m,
        "GhosttyFormatterTerminalOptions",
        size_of::<GhosttyFormatterTerminalOptions>(),
        &[
            ("emit", offset_of!(GhosttyFormatterTerminalOptions, emit)),
            (
                "unwrap",
                offset_of!(GhosttyFormatterTerminalOptions, unwrap),
            ),
            ("trim", offset_of!(GhosttyFormatterTerminalOptions, trim)),
            ("extra", offset_of!(GhosttyFormatterTerminalOptions, extra)),
            (
                "selection",
                offset_of!(GhosttyFormatterTerminalOptions, selection),
            ),
        ],
    );
    for (ty, size) in [
        ("GhosttyCell", size_of::<GhosttyCell>()),
        ("GhosttyRow", size_of::<GhosttyRow>()),
        ("GhosttyMods", size_of::<GhosttyMods>()),
        ("GhosttyMode", size_of::<GhosttyMode>()),
    ] {
        assert_eq!(
            m["types"][ty]["size"].as_u64(),
            Some(size as u64),
            "{ty} size"
        );
    }
}

#[test]
fn enum_constants_match_the_manifest() {
    let m = manifest();
    let v = |ty: &str, name: &str, got: i32| value(&m, ty, name, i64::from(got));
    v("GhosttyResult", "SUCCESS", GHOSTTY_SUCCESS);
    v("GhosttyResult", "OUT_OF_MEMORY", GHOSTTY_OUT_OF_MEMORY);
    v("GhosttyResult", "INVALID_VALUE", GHOSTTY_INVALID_VALUE);
    v("GhosttyResult", "OUT_OF_SPACE", GHOSTTY_OUT_OF_SPACE);
    v("GhosttyResult", "NO_VALUE", GHOSTTY_NO_VALUE);
    v("GhosttyResult", "IO_ERROR", GHOSTTY_IO_ERROR);
    v("GhosttyResult", "LIMIT_EXCEEDED", GHOSTTY_LIMIT_EXCEEDED);
    v("GhosttyResult", "REJECTED", GHOSTTY_REJECTED);
    v("GhosttyStyleColorTag", "NONE", GHOSTTY_STYLE_COLOR_NONE);
    v(
        "GhosttyStyleColorTag",
        "PALETTE",
        GHOSTTY_STYLE_COLOR_PALETTE,
    );
    v("GhosttyStyleColorTag", "RGB", GHOSTTY_STYLE_COLOR_RGB);
    for (name, got) in [
        ("USERDATA", GHOSTTY_TERMINAL_OPT_USERDATA),
        ("WRITE_PTY", GHOSTTY_TERMINAL_OPT_WRITE_PTY),
        ("BELL", GHOSTTY_TERMINAL_OPT_BELL),
        ("XTVERSION", GHOSTTY_TERMINAL_OPT_XTVERSION),
        ("TITLE_CHANGED", GHOSTTY_TERMINAL_OPT_TITLE_CHANGED),
        ("SIZE", GHOSTTY_TERMINAL_OPT_SIZE),
        ("COLOR_SCHEME", GHOSTTY_TERMINAL_OPT_COLOR_SCHEME),
        ("COLOR_FOREGROUND", GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND),
        ("COLOR_BACKGROUND", GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND),
        ("COLOR_CURSOR", GHOSTTY_TERMINAL_OPT_COLOR_CURSOR),
        ("COLOR_PALETTE", GHOSTTY_TERMINAL_OPT_COLOR_PALETTE),
        (
            "KITTY_IMAGE_STORAGE_LIMIT",
            GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
        ),
        ("GLYPH_PROTOCOL", GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL),
        ("PWD_CHANGED", GHOSTTY_TERMINAL_OPT_PWD_CHANGED),
        ("CLIPBOARD_WRITE", GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE),
        (
            "SCROLLBACK_MAX_BYTES",
            GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
        ),
        (
            "SCROLLBACK_MAX_LINES",
            GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
        ),
        (
            "DESKTOP_NOTIFICATION",
            GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
        ),
        ("PROGRESS_REPORT", GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT),
        (
            "CONTINUATION_MAX_BYTES",
            GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
        ),
        ("MODE_DEFAULT", GHOSTTY_TERMINAL_OPT_MODE_DEFAULT),
        ("MODE", GHOSTTY_TERMINAL_OPT_MODE),
        ("TERMINFO_NAME", GHOSTTY_TERMINAL_OPT_TERMINFO_NAME),
        (
            "CLIPBOARD_WRITE_MAX_BYTES",
            GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES,
        ),
    ] {
        v("GhosttyTerminalOption", name, got);
    }
    for (name, got) in [
        ("COLS", GHOSTTY_TERMINAL_DATA_COLS),
        ("ROWS", GHOSTTY_TERMINAL_DATA_ROWS),
        ("ACTIVE_SCREEN", GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN),
        ("CURSOR_VISIBLE", GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE),
        (
            "KITTY_KEYBOARD_FLAGS",
            GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
        ),
        ("MOUSE_TRACKING", GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING),
        ("TITLE", GHOSTTY_TERMINAL_DATA_TITLE),
        ("PWD", GHOSTTY_TERMINAL_DATA_PWD),
        ("TOTAL_ROWS", GHOSTTY_TERMINAL_DATA_TOTAL_ROWS),
        ("SCROLLBACK_ROWS", GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS),
        (
            "CONTINUATION_MAX_BYTES",
            GHOSTTY_TERMINAL_DATA_CONTINUATION_MAX_BYTES,
        ),
        ("MODE", GHOSTTY_TERMINAL_DATA_MODE),
    ] {
        v("GhosttyTerminalData", name, got);
    }
    v(
        "GhosttyTerminalScreen",
        "PRIMARY",
        GHOSTTY_TERMINAL_SCREEN_PRIMARY,
    );
    v(
        "GhosttyTerminalScreen",
        "ALTERNATE",
        GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
    );
    v(
        "GhosttyTerminalCompressionMode",
        "INCREMENTAL",
        GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL,
    );
    v(
        "GhosttyTerminalCompressionResult",
        "UNSUPPORTED",
        GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED,
    );
    v(
        "GhosttyTerminalCompressionResult",
        "PENDING",
        GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING,
    );
    v(
        "GhosttyTerminalCompressionResult",
        "COMPLETE",
        GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE,
    );
    v(
        "GhosttySnapshotDecoderOption",
        "MAX_CONTINUATION_BYTES",
        GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
    );
    v(
        "GhosttySnapshotDecoderOption",
        "RETAIN_CONTINUATION",
        GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
    );
    v("GhosttyColorScheme", "LIGHT", GHOSTTY_COLOR_SCHEME_LIGHT);
    v("GhosttyColorScheme", "DARK", GHOSTTY_COLOR_SCHEME_DARK);
    v(
        "GhosttyClipboardLocation",
        "STANDARD",
        GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
    );
    v(
        "GhosttyClipboardWriteResult",
        "SUCCESS",
        GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
    );
    v(
        "GhosttyRenderStateData",
        "COLS",
        GHOSTTY_RENDER_STATE_DATA_COLS,
    );
    v(
        "GhosttyRenderStateData",
        "ROWS",
        GHOSTTY_RENDER_STATE_DATA_ROWS,
    );
    v(
        "GhosttyRenderStateData",
        "DIRTY",
        GHOSTTY_RENDER_STATE_DATA_DIRTY,
    );
    v(
        "GhosttyRenderStateData",
        "ROW_ITERATOR",
        GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
    );
    v(
        "GhosttyRenderStateData",
        "CURSOR",
        GHOSTTY_RENDER_STATE_DATA_CURSOR,
    );
    v(
        "GhosttyRenderStateDirty",
        "FALSE",
        GHOSTTY_RENDER_STATE_DIRTY_FALSE,
    );
    v(
        "GhosttyRenderStateDirty",
        "PARTIAL",
        GHOSTTY_RENDER_STATE_DIRTY_PARTIAL,
    );
    v(
        "GhosttyRenderStateDirty",
        "FULL",
        GHOSTTY_RENDER_STATE_DIRTY_FULL,
    );
    v(
        "GhosttyRenderStateRowData",
        "RAW",
        GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
    );
    v(
        "GhosttyRenderStateRowData",
        "CELLS",
        GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
    );
    v(
        "GhosttyRenderStateRowData",
        "CELLS_RAW",
        GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW,
    );
    v(
        "GhosttyRenderStateRowCellsData",
        "STYLE",
        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
    );
    v(
        "GhosttyRenderStateRowCellsData",
        "GRAPHEMES_LEN",
        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
    );
    v(
        "GhosttyRenderStateRowCellsData",
        "GRAPHEMES_BUF",
        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
    );
    v("GhosttyCellData", "CODEPOINT", GHOSTTY_CELL_DATA_CODEPOINT);
    v(
        "GhosttyCellData",
        "CONTENT_TAG",
        GHOSTTY_CELL_DATA_CONTENT_TAG,
    );
    v("GhosttyCellData", "WIDE", GHOSTTY_CELL_DATA_WIDE);
    v(
        "GhosttyCellData",
        "HAS_STYLING",
        GHOSTTY_CELL_DATA_HAS_STYLING,
    );
    v(
        "GhosttyCellData",
        "COLOR_PALETTE",
        GHOSTTY_CELL_DATA_COLOR_PALETTE,
    );
    v("GhosttyCellData", "COLOR_RGB", GHOSTTY_CELL_DATA_COLOR_RGB);
    v(
        "GhosttyCellContentTag",
        "CODEPOINT",
        GHOSTTY_CELL_CONTENT_CODEPOINT,
    );
    v(
        "GhosttyCellContentTag",
        "CODEPOINT_GRAPHEME",
        GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME,
    );
    v(
        "GhosttyCellContentTag",
        "BG_COLOR_PALETTE",
        GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE,
    );
    v(
        "GhosttyCellContentTag",
        "BG_COLOR_RGB",
        GHOSTTY_CELL_CONTENT_BG_COLOR_RGB,
    );
    v("GhosttyCellWide", "NARROW", GHOSTTY_CELL_WIDE_NARROW);
    v("GhosttyCellWide", "WIDE", GHOSTTY_CELL_WIDE_WIDE);
    v(
        "GhosttyCellWide",
        "SPACER_TAIL",
        GHOSTTY_CELL_WIDE_SPACER_TAIL,
    );
    v(
        "GhosttyCellWide",
        "SPACER_HEAD",
        GHOSTTY_CELL_WIDE_SPACER_HEAD,
    );
    v("GhosttyRowData", "WRAP", GHOSTTY_ROW_DATA_WRAP);
    v("GhosttyPointTag", "ACTIVE", GHOSTTY_POINT_TAG_ACTIVE);
    v("GhosttyPointTag", "SCREEN", GHOSTTY_POINT_TAG_SCREEN);
    v("GhosttyPointTag", "HISTORY", GHOSTTY_POINT_TAG_HISTORY);
    v("GhosttySearchOption", "NEEDLE", GHOSTTY_SEARCH_OPT_NEEDLE);
    v(
        "GhosttySearchOption",
        "SELECT_SCROLL",
        GHOSTTY_SEARCH_OPT_SELECT_SCROLL,
    );
    v(
        "GhosttySearchData",
        "TOTAL_MATCHES",
        GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
    );
    v("GhosttySearchData", "MATCHES", GHOSTTY_SEARCH_DATA_MATCHES);
    v("GhosttySearchScroll", "NONE", GHOSTTY_SEARCH_SCROLL_NONE);
    v("GhosttyKey", "UNIDENTIFIED", GHOSTTY_KEY_UNIDENTIFIED);
    v("GhosttyKey", "ENTER", GHOSTTY_KEY_ENTER);
    v(
        "GhosttyFormatterFormat",
        "PLAIN",
        GHOSTTY_FORMATTER_FORMAT_PLAIN,
    );
    v(
        "GhosttyKeyEncoderOption",
        "MACOS_OPTION_AS_ALT",
        GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
    );
    v("GhosttyOptionAsAlt", "FALSE", GHOSTTY_OPTION_AS_ALT_FALSE);
    v("GhosttyOptionAsAlt", "TRUE", GHOSTTY_OPTION_AS_ALT_TRUE);
    v("GhosttyOptionAsAlt", "LEFT", GHOSTTY_OPTION_AS_ALT_LEFT);
    v("GhosttyOptionAsAlt", "RIGHT", GHOSTTY_OPTION_AS_ALT_RIGHT);
    v(
        "GhosttyMouseEncoderOption",
        "SIZE",
        GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
    );
    v(
        "GhosttyMouseEncoderOption",
        "ANY_BUTTON_PRESSED",
        GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
    );
    v(
        "GhosttyMouseEncoderOption",
        "TRACK_LAST_CELL",
        GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
    );
    v("GhosttyFocusEvent", "GAINED", GHOSTTY_FOCUS_GAINED);
    v("GhosttyFocusEvent", "LOST", GHOSTTY_FOCUS_LOST);
    v(
        "GhosttyPasteSource",
        "CLIPBOARD",
        GHOSTTY_PASTE_SOURCE_CLIPBOARD,
    );
    v("GhosttySysOption", "LOG", GHOSTTY_SYS_OPT_LOG);
}

#[test]
fn ply_proto_numbering_matches_the_library() {
    let m = manifest();
    let v = |ty: &str, name: &str, got: i64| value(&m, ty, name, got);
    // ply-proto's C2 enums are numbered as libghostty-vt's; ply-term casts them straight through.
    for (name, got) in [("RELEASE", 0), ("PRESS", 1), ("REPEAT", 2)] {
        v("GhosttyKeyAction", name, got);
    }
    for (name, got) in [("PRESS", 0), ("RELEASE", 1), ("MOTION", 2)] {
        v("GhosttyMouseAction", name, got);
    }
    for (name, got) in [
        ("UNKNOWN", 0),
        ("LEFT", 1),
        ("RIGHT", 2),
        ("MIDDLE", 3),
        ("FOUR", 4),
        ("ELEVEN", 11),
    ] {
        v("GhosttyMouseButton", name, got);
    }
    for (name, got) in [
        ("BAR", 0),
        ("BLOCK", 1),
        ("UNDERLINE", 2),
        ("BLOCK_HOLLOW", 3),
    ] {
        v("GhosttyRenderStateCursorVisualStyle", name, got);
    }
    for (name, got) in [
        ("NONE", 0),
        ("SINGLE", 1),
        ("DOUBLE", 2),
        ("CURLY", 3),
        ("DOTTED", 4),
        ("DASHED", 5),
    ] {
        v("GhosttySgrUnderline", name, got);
    }
    let keys = m["types"]["GhosttyKey"]["values"]
        .as_object()
        .expect("GhosttyKey values");
    let max = keys
        .iter()
        .filter(|(k, _)| *k != "MAX_VALUE")
        .filter_map(|(_, v)| v.as_i64())
        .max();
    assert_eq!(
        max,
        Some(i64::from(GHOSTTY_KEY_MAX)),
        "the highest defined key"
    );
}
