//! Settings kept between runs in ~/.config/pingerer/config.json (or
//! $XDG_CONFIG_HOME/pingerer, or $PINGERER_CONFIG): the API token, the
//! server's URL and the account in use. The file is readable by its owner
//! only (0600), since it holds the token.
//!
//! PINGERER_TOKEN and PINGERER_URL win over the file, so CI needs no login.

use crate::errors::{failed, Result};
use crate::DEFAULT_URL;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Config {
    pub path: PathBuf,
    env: HashMap<String, String>,
    values: Map<String, Value>,
}

impl Config {
    pub fn default_path(env: &HashMap<String, String>) -> PathBuf {
        if let Some(path) = env.get("PINGERER_CONFIG").filter(|it| !it.is_empty()) {
            return PathBuf::from(path);
        }
        let base = match env.get("XDG_CONFIG_HOME").filter(|it| !it.is_empty()) {
            Some(base) => PathBuf::from(base),
            None => home(env).join(".config"),
        };
        base.join("pingerer").join("config.json")
    }

    pub fn new(path: PathBuf, env: HashMap<String, String>) -> Self {
        let values = read(&path);
        Config { path, env, values }
    }

    pub fn token(&self) -> Option<String> {
        self.env_value("PINGERER_TOKEN").or_else(|| self.values.get("token").and_then(Value::as_str).map(String::from))
    }

    pub fn url(&self) -> String {
        let url = self.env_value("PINGERER_URL").or_else(|| self.values.get("url").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| DEFAULT_URL.into());
        url.trim_end_matches('/').to_string()
    }

    pub fn account(&self) -> Option<String> {
        self.values.get("account").filter(|it| !it.is_null()).map(crate::values::s)
    }

    pub fn token_from_env(&self) -> bool {
        self.env_value("PINGERER_TOKEN").is_some()
    }

    /// Sets each key; None removes it. Writes the file at once.
    pub fn update(&mut self, changes: Vec<(&str, Option<String>)>) -> Result<()> {
        for (key, value) in changes {
            match value {
                Some(value) => self.values.insert(key.to_string(), Value::String(value)),
                None => self.values.remove(key),
            };
        }
        self.write()
    }

    fn env_value(&self, name: &str) -> Option<String> {
        self.env.get(name).map(|it| it.trim().to_string()).filter(|it| !it.is_empty())
    }

    fn write(&self) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let written = (|| -> std::io::Result<()> {
            create_private_dir(dir)?;
            let tmp = self.path.with_extension(format!("json.{}.tmp", std::process::id()));
            write_private(&tmp, serde_json::to_string_pretty(&Value::Object(self.values.clone())).unwrap_or_default().as_bytes())?;
            std::fs::rename(&tmp, &self.path)
        })();
        written.or_else(|error| failed(format!("Couldn't save {}: {error}", self.path.display())))
    }
}

fn read(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| value.as_object().cloned()).unwrap_or_default()
}

fn home(env: &HashMap<String, String>) -> PathBuf {
    env.get("HOME").or_else(|| env.get("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.exists() {
        return Ok(());
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(data)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}
