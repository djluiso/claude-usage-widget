//! User settings, stored as JSON in %LOCALAPPDATA%\ClaudeUsageWidget\config.json.
//! The file is created with defaults on first run and re-read when it changes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// USD per million tokens: [input, output, cache write, cache read].
pub type Price = [f64; 4];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// "left" = next to the Widgets button, "right" = next to the tray icons.
    pub dock: String,
    /// "system" (follow Windows light/dark mode), "dark" or "light".
    pub theme: String,
    /// How often transcripts are rescanned, in milliseconds.
    pub refresh_ms: u32,
    /// API-equivalent price estimates, matched by substring of the model id.
    pub prices: BTreeMap<String, Price>,
}

impl Default for Config {
    fn default() -> Self {
        let mut prices = BTreeMap::new();
        prices.insert("opus".into(), [5.00, 25.00, 6.25, 0.50]);
        prices.insert("sonnet".into(), [3.00, 15.00, 3.75, 0.30]);
        prices.insert("haiku".into(), [1.00, 5.00, 1.25, 0.10]);
        prices.insert("fable".into(), [5.00, 25.00, 6.25, 0.50]);
        Self { dock: "left".into(), theme: "system".into(), refresh_ms: 3000, prices }
    }
}

impl Config {
    pub fn price_for(&self, model: &str) -> Price {
        let m = model.to_lowercase();
        self.prices
            .iter()
            .find(|(k, _)| m.contains(k.as_str()))
            .map(|(_, p)| *p)
            .or_else(|| self.prices.get("sonnet").copied())
            .unwrap_or([3.0, 15.0, 3.75, 0.30])
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("ClaudeUsageWidget");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn log_path() -> PathBuf {
    data_dir().join("error.log")
}

/// Claude Code's transcripts folder (honours CLAUDE_CONFIG_DIR).
pub fn projects_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("projects");
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".claude").join("projects")
}

pub struct ConfigFile {
    pub cfg: Config,
    mtime: Option<SystemTime>,
}

impl ConfigFile {
    pub fn load() -> Self {
        let path = config_path();
        let cfg = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let mut me = Self { cfg: cfg.clone().unwrap_or_default(), mtime: None };
        if cfg.is_none() {
            me.save();
        }
        me.mtime = mtime(&path);
        me
    }

    pub fn save(&mut self) {
        let path = config_path();
        if let Ok(s) = serde_json::to_string_pretty(&self.cfg) {
            let _ = std::fs::write(&path, s);
        }
        self.mtime = mtime(&path);
    }

    /// Re-read the file if it was edited. Returns true when it changed.
    pub fn reload_if_changed(&mut self) -> bool {
        let path = config_path();
        let m = mtime(&path);
        if m == self.mtime {
            return false;
        }
        self.mtime = m;
        match std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()) {
            Some(cfg) => {
                self.cfg = cfg;
                true
            }
            None => false, // keep the last good config while the user is mid-edit
        }
    }
}

fn mtime(p: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}
