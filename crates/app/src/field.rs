//! A single text value with a caret, edited from key events the workspace routes to it.

#[derive(Default, Clone)]
pub struct Field {
    pub text: String,
    caret: usize,
}

impl Field {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            caret: text.len(),
            text,
        }
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.len();
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    /// Applies an editing key; returns whether the text or caret changed.
    pub fn key(&mut self, key: &str, alt: bool) -> bool {
        match key {
            "backspace" if self.caret > 0 => {
                let from = if alt { self.word_start() } else { self.prev() };
                self.text.replace_range(from..self.caret, "");
                self.caret = from;
            }
            "delete" if self.caret < self.text.len() => {
                let to = self.next();
                self.text.replace_range(self.caret..to, "");
            }
            "left" => self.caret = self.prev(),
            "right" => self.caret = self.next(),
            "home" | "up" => self.caret = 0,
            "end" | "down" => self.caret = self.text.len(),
            _ => return false,
        }
        true
    }

    fn word_start(&self) -> usize {
        let head = &self.text[..self.caret];
        let trimmed = head.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed
            .rfind(|c: char| !c.is_alphanumeric())
            .map_or(0, |i| i + 1)
    }

    /// The text split at the caret, for drawing the caret between the halves.
    pub fn halves(&self) -> (&str, &str) {
        self.text.split_at(self.caret)
    }
}
