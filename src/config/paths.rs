//! Config file path resolution and auto-discovery.
//!
//! Priority when resolving which config file to load:
//! 1. `--config <path>` CLI flag
//! 2. `RAUSU_CONFIG` environment variable
//! 3. `./config.yaml`
//! 4. `./rausu-config.yaml`
//! 5. `${XDG_CONFIG_HOME}/rausu/config.yaml`
//! 6. `${XDG_CONFIG_HOME}/rausu/rausu-config.yaml`
//! 7. `~/.rausu/config.yaml`
//! 8. `~/rausu-config.yaml`

use std::path::PathBuf;

/// Returns the XDG config home directory, falling back to `~/.config`.
fn xdg_config_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
}

/// Returns the default path for the Rausu config file:
/// `${XDG_CONFIG_HOME:-~/.config}/rausu/config.yaml`.
///
/// This is also the location used by `rausu init` when no `--path` is given.
pub fn default_config_path() -> PathBuf {
    xdg_config_home().join("rausu").join("config.yaml")
}

/// All candidate config paths checked during auto-discovery (steps 3–8 in the
/// priority order).  Only paths that actually exist are returned by
/// [`resolve_config_path`].
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("config.yaml"),
        PathBuf::from("rausu-config.yaml"),
    ];
    let xdg = xdg_config_home();
    paths.push(xdg.join("rausu/config.yaml"));
    paths.push(xdg.join("rausu/rausu-config.yaml"));
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".rausu/config.yaml"));
        paths.push(home.join("rausu-config.yaml"));
    }
    paths
}

/// Resolve the config file path from CLI arg, `RAUSU_CONFIG` env var, or
/// auto-discovery.  Returns `None` if no existing config file was found and
/// neither CLI nor env-var override was provided.
///
/// When a CLI path or `RAUSU_CONFIG` value is given it is returned as-is
/// without checking for existence — [`crate::config::AppConfig::load`] handles
/// missing files gracefully with defaults.
pub fn resolve_config_path(cli_path: Option<&str>) -> Option<PathBuf> {
    // 1. CLI --config (returned as-is; existence is not required)
    if let Some(p) = cli_path {
        return Some(PathBuf::from(p));
    }
    // 2. RAUSU_CONFIG env var (returned as-is; existence is not required)
    if let Ok(p) = std::env::var("RAUSU_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    // 3–8. Auto-discovery: only return a path if the file exists
    candidate_paths().into_iter().find(|path| path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_cli_path_returned_as_is() {
        let result = resolve_config_path(Some("/explicit/path.yaml"));
        assert_eq!(result, Some(PathBuf::from("/explicit/path.yaml")));
    }

    #[test]
    fn test_resolve_env_var() {
        // Safety: single-threaded test; other tests should not read RAUSU_CONFIG
        // at the same moment.
        std::env::set_var("RAUSU_CONFIG", "/from/env.yaml");
        let result = resolve_config_path(None);
        std::env::remove_var("RAUSU_CONFIG");
        assert_eq!(result, Some(PathBuf::from("/from/env.yaml")));
    }

    #[test]
    fn test_cli_path_takes_priority_over_env_var() {
        std::env::set_var("RAUSU_CONFIG", "/from/env.yaml");
        let result = resolve_config_path(Some("/from/cli.yaml"));
        std::env::remove_var("RAUSU_CONFIG");
        assert_eq!(result, Some(PathBuf::from("/from/cli.yaml")));
    }

    #[test]
    fn test_default_config_path_contains_rausu() {
        let path = default_config_path();
        assert!(
            path.to_string_lossy().contains("rausu"),
            "default path should contain 'rausu': {path:?}"
        );
        assert!(path.ends_with("config.yaml"));
    }
}
