//! First-run seeding of the Lexi home (`~/.lexi-cli/`).
//!
//! Writes the shipped defaults — `providers/providers-config.yaml`, each
//! provider's model files, and `aliases.json` — **only where they are missing**.
//! Existing files are never overwritten: user edits always win. (The Python CLI
//! replaced a differing `providers-config.yaml` with the packaged copy on every
//! run, keeping a `.bak`; that is deliberately not carried over.)
//!
//! API definitions are not seeded: the runtime resolves shipped definitions from
//! the copies embedded in the binary, and `<home>/apis/` holds custom ones only.

use std::path::{Path, PathBuf};

use crate::config::lexi_home;
use crate::HostError;

/// The defaults embedded in the binary, as (path relative to the home, contents).
const DEFAULTS: &[(&str, &str)] = &[
    (
        "providers/providers-config.yaml",
        include_str!("../../providers/providers-config.yaml"),
    ),
    (
        "providers/anthropic/configured-models.yaml",
        include_str!("../../providers/anthropic/configured-models.yaml"),
    ),
    (
        "providers/anthropic/default-model.yaml",
        include_str!("../../providers/anthropic/default-model.yaml"),
    ),
    (
        "providers/anthropic/model-aliases.yaml",
        include_str!("../../providers/anthropic/model-aliases.yaml"),
    ),
    (
        "providers/nvidia/configured-models.yaml",
        include_str!("../../providers/nvidia/configured-models.yaml"),
    ),
    (
        "providers/nvidia/default-model.yaml",
        include_str!("../../providers/nvidia/default-model.yaml"),
    ),
    (
        "providers/nvidia/model-aliases.yaml",
        include_str!("../../providers/nvidia/model-aliases.yaml"),
    ),
    (
        "providers/openai/configured-models.yaml",
        include_str!("../../providers/openai/configured-models.yaml"),
    ),
    (
        "providers/openai/default-model.yaml",
        include_str!("../../providers/openai/default-model.yaml"),
    ),
    (
        "providers/openai/model-aliases.yaml",
        include_str!("../../providers/openai/model-aliases.yaml"),
    ),
];

/// The default command aliases (`aliases.json`).
const DEFAULT_ALIASES: &str = include_str!("../../providers/default-aliases.json");

/// The shipped default command aliases (`aliases.json` contents).
pub fn default_aliases() -> &'static str {
    DEFAULT_ALIASES
}

/// Seed the Lexi home (`$LEXI_HOME` or `~/.lexi-cli`). See [`ensure_home_in`].
pub fn ensure_home() -> Result<Vec<PathBuf>, HostError> {
    ensure_home_in(&lexi_home())
}

/// Seed `home` with any missing defaults; returns the files it created
/// (empty when the home is already complete).
pub fn ensure_home_in(home: &Path) -> Result<Vec<PathBuf>, HostError> {
    let mut created = Vec::new();
    std::fs::create_dir_all(home.join("prompts"))?;
    for (rel, contents) in DEFAULTS {
        let path = home.join(rel);
        if !path.exists() {
            write_new(&path, contents)?;
            created.push(path);
        }
    }
    // Aliases: seed when missing, or when present but holding no aliases (an
    // empty object / blank file), matching the Python CLI. Unparseable files
    // are left alone rather than clobbered.
    let aliases = home.join("aliases.json");
    if aliases_missing_or_empty(&aliases) {
        write_new(&aliases, DEFAULT_ALIASES)?;
        created.push(aliases);
    }
    crate::config::upgrade_key_sources(home);
    let moved = crate::config::move_saved_keys(home);
    if !moved.is_empty() {
        if let Ok(mut n) = notices().lock() {
            n.extend(moved);
        }
    }
    Ok(created)
}

fn notices() -> &'static std::sync::Mutex<Vec<String>> {
    static NOTICES: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    NOTICES.get_or_init(Default::default)
}

/// What [`ensure_home`] did that's worth telling you once ("Moved
/// anthropic's API key into the macOS Keychain."), taken: the next call
/// returns only newer ones. The CLI prints them, the desktop app shows them
/// at startup, the Python library issues them as warnings.
pub fn take_notices() -> Vec<String> {
    notices()
        .lock()
        .map(|mut n| std::mem::take(&mut *n))
        .unwrap_or_default()
}

fn aliases_missing_or_empty(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return !path.exists();
    };
    if text.trim().is_empty() {
        return true;
    }
    matches!(
        serde_json::from_str::<serde_json::Value>(&text),
        Ok(serde_json::Value::Object(m)) if m.is_empty()
    )
}

fn write_new(path: &Path, contents: &str) -> Result<(), HostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_an_empty_home() {
        let dir = tempfile::tempdir().unwrap();
        let created = ensure_home_in(dir.path()).unwrap();
        assert_eq!(created.len(), DEFAULTS.len() + 1);
        assert!(dir.path().join("prompts").is_dir());
        let cfg =
            std::fs::read_to_string(dir.path().join("providers/providers-config.yaml")).unwrap();
        assert!(cfg.contains("anthropic:"));
        let aliases: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("aliases.json")).unwrap(),
        )
        .unwrap();
        assert!(aliases.get("exit").is_some());
    }

    #[test]
    fn is_idempotent_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        ensure_home_in(dir.path()).unwrap();
        let cfg = dir.path().join("providers/providers-config.yaml");
        std::fs::write(&cfg, "providers: {mine: {}}\n").unwrap();
        let aliases = dir.path().join("aliases.json");
        std::fs::write(&aliases, r#"{"help": ["?"]}"#).unwrap();

        let created = ensure_home_in(dir.path()).unwrap();
        assert!(created.is_empty());
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "providers: {mine: {}}\n"
        );
        assert_eq!(
            std::fs::read_to_string(&aliases).unwrap(),
            r#"{"help": ["?"]}"#
        );
    }

    #[test]
    fn reseeds_empty_aliases_but_not_broken_ones() {
        let dir = tempfile::tempdir().unwrap();
        ensure_home_in(dir.path()).unwrap();
        let aliases = dir.path().join("aliases.json");

        std::fs::write(&aliases, "{}").unwrap();
        assert_eq!(ensure_home_in(dir.path()).unwrap(), vec![aliases.clone()]);

        std::fs::write(&aliases, "{not json").unwrap();
        assert!(ensure_home_in(dir.path()).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&aliases).unwrap(), "{not json");
    }
}
