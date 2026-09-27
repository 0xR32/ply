//! Which pasted text Claude Code reads images for: its paste handler's own rule, read from Claude Code 2.1.283.

const IMAGE_EXTENSIONS: [&str; 5] = [".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// Whether Claude Code reads an image for part of `text` when it arrives as a paste: the text split at line breaks and at a space before `/` or a drive letter, a piece that, trimmed, stripped of one pair of matching quotes and unescaped, ends in `.png`, `.jpg`, `.jpeg`, `.gif` or `.webp` (any case); it reads such a piece even when no such file exists.
pub fn reads_images(text: &str) -> bool {
    text.split('\n')
        .flat_map(pieces)
        .any(|piece| is_image_path(piece.trim()))
}

fn pieces(line: &str) -> Vec<&str> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let rest = &bytes[i + 1..];
        let drive = matches!(rest, [letter, b':', b'\\', ..] if letter.is_ascii_alphabetic());
        if b == b' ' && (rest.first() == Some(&b'/') || drive) {
            out.push(&line[start..i]);
            start = i + 1;
        }
    }
    out.push(&line[start..]);
    out
}

fn is_image_path(piece: &str) -> bool {
    let unquoted = ['"', '\'']
        .into_iter()
        .find_map(|q| piece.strip_prefix(q)?.strip_suffix(q))
        .unwrap_or(piece);
    let path = unescape(unquoted).to_ascii_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            chars.next().unwrap_or('\\')
        } else {
            c
        });
    }
    out
}
