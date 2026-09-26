//! `config.toml` (spec 11.1): the user settings of `settings.get`/`settings.set` and the last `theme.set` palette.
//!
//! Every key is optional; a missing key takes the [`Settings`] default (`scrollback_lines` 10 000,
//! `keep_awake_while_running`, `resume_sessions_on_start` and `use_ply_colours_in_claude` on, `option_as_meta` off). The palette is kept here so
//! a restarted plyd can spawn panes before the app sends `theme.set` again (R-R4: no child starts before its
//! terminal knows the palette). A file that does not parse is logged and ignored, never overwritten until the next
//! `settings.set` or `theme.set`. Writes go to a temporary file renamed over the old one, mode 0600.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use ply_proto::pane::{AccentName, OptionAsMeta, Settings, TerminalTheme};
use serde::{Deserialize, Serialize};

use crate::error::{Result, io};

/// The contents of `config.toml`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    /// User settings, every field filled.
    pub settings: Settings,
    /// The palette of the last `theme.set`; `None` until the app sent one.
    pub palette: Option<TerminalTheme>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accent: Option<AccentName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    option_as_meta: Option<OptionAsMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_awake_while_running: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_sessions_on_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    use_ply_colours_in_claude: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex_plan_tool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scrollback_lines: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    font_size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    palette: Option<TerminalTheme>,
}

impl Config {
    /// Parses `text`; missing keys take their defaults. Fails with the TOML error for bad syntax, unknown keys or bad values.
    pub fn parse(text: &str) -> std::result::Result<Self, toml::de::Error> {
        let file: ConfigFile = toml::from_str(text)?;
        let d = Settings::default();
        Ok(Self {
            settings: Settings {
                accent: file.accent.unwrap_or(d.accent),
                option_as_meta: file.option_as_meta.unwrap_or(d.option_as_meta),
                keep_awake_while_running: file
                    .keep_awake_while_running
                    .unwrap_or(d.keep_awake_while_running),
                resume_sessions_on_start: file
                    .resume_sessions_on_start
                    .unwrap_or(d.resume_sessions_on_start),
                use_ply_colours_in_claude: file
                    .use_ply_colours_in_claude
                    .unwrap_or(d.use_ply_colours_in_claude),
                codex_plan_tool: file.codex_plan_tool.unwrap_or(d.codex_plan_tool),
                scrollback_lines: file.scrollback_lines.unwrap_or(d.scrollback_lines),
                font_size: file.font_size.unwrap_or(d.font_size),
            },
            palette: file.palette,
        })
    }

    /// Serialises every setting and the palette, if any; fails only if the TOML serialiser does.
    pub fn render(&self) -> Result<String> {
        let s = &self.settings;
        let file = ConfigFile {
            accent: Some(s.accent),
            option_as_meta: Some(s.option_as_meta),
            keep_awake_while_running: Some(s.keep_awake_while_running),
            resume_sessions_on_start: Some(s.resume_sessions_on_start),
            use_ply_colours_in_claude: Some(s.use_ply_colours_in_claude),
            codex_plan_tool: Some(s.codex_plan_tool),
            scrollback_lines: Some(s.scrollback_lines),
            font_size: Some(s.font_size),
            palette: self.palette.clone(),
        };
        Ok(toml::to_string(&file)?)
    }

    /// Reads `path`; a missing file gives the defaults, and an unreadable or invalid one is logged and gives the defaults too.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => match Self::parse(&text) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "config.toml is invalid; using the defaults");
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read config.toml; using the defaults");
                Self::default()
            }
        }
    }

    /// Writes the file atomically (temporary file, then rename) with mode 0600; blocks on the filesystem.
    /// Fails with [`crate::Error::Io`] or [`crate::Error::ConfigWrite`].
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.render()?;
        let tmp = path.with_extension("toml.tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(io("cannot create", &tmp))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(io("cannot write", &tmp))?;
        fs::rename(&tmp, path).map_err(io("cannot replace", path))
    }
}

#[cfg(test)]
mod tests {
    use ply_proto::pane::Rgb;

    use super::*;

    fn theme() -> TerminalTheme {
        let grey = |v: u8| Rgb { r: v, g: v, b: v };
        TerminalTheme {
            ansi: [grey(0x80); 16],
            fg: grey(0xe6),
            bg: grey(0x0c),
            cursor: grey(0xf0),
            cursor_text: grey(0x0c),
            selection_bg: grey(0x30),
            selection_fg: grey(0xe6),
        }
    }

    #[test]
    fn an_empty_file_gives_the_spec_defaults() {
        let c = Config::parse("").unwrap();
        assert_eq!(c.settings.scrollback_lines, 10_000);
        assert!(c.settings.keep_awake_while_running);
        assert!(c.settings.use_ply_colours_in_claude);
        assert_eq!(c.settings.option_as_meta, OptionAsMeta::Off);
        assert_eq!(c.palette, None);
    }

    #[test]
    fn settings_and_palette_round_trip() {
        let mut c = Config::default();
        c.settings.option_as_meta = OptionAsMeta::Left;
        c.settings.scrollback_lines = 2_000;
        c.palette = Some(theme());
        assert_eq!(Config::parse(&c.render().unwrap()).unwrap(), c);
    }

    #[test]
    fn unknown_keys_are_rejected_and_load_falls_back() {
        assert!(Config::parse("scrolback_lines = 5\n").is_err());
        let dir = std::env::temp_dir().join(format!("ply-config-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "not toml at all [").unwrap();
        assert_eq!(Config::load(&path), Config::default());
        let mut c = Config::default();
        c.settings.keep_awake_while_running = false;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path), c);
        fs::remove_dir_all(&dir).unwrap();
    }
}
