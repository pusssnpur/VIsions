//! Where the user's files live.
//!
//! Resolution order for each value: environment variable, then the config
//! file (`VICTORY_CONFIG`, else `./victory.toml`), then the default.
//! See `victory.example.toml` at the repository root.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::policy::ensure_outside_repo;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuConfig {
    /// Target executable path inside `game_dir` (optional convenience for the launcher).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_exe: Option<PathBuf>,
    /// Console profile (`"series-x"` or `"series-s"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console: Option<String>,
    /// AMD FSR 1.0 preset (`"off"`, `"ultra-quality"`, `"quality"`, `"balanced"`, `"performance"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsr: Option<String>,
    /// FSR RCAS sharpness stops (`0.0` = sharpest .. `2.0` = softest; default `0.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsr_sharpness: Option<f32>,
    /// Optional explicit output resolution for FSR upscaling (`"1920x1080"`, `"2560x1440"`, `"3840x2160"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsr_resolution: Option<String>,
    /// Host VRAM budget cap for placed/streamed textures in MiB (`0` = automatic 75% of adapter budget).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_budget_mb: Option<u64>,
    /// Maximum side length of streamed block-compressed textures (`0` = automatic by VRAM pressure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_max: Option<u64>,
    /// Device-local VRAM card buffer pool cap in MiB (`0` disables VRAM card buffers; default `512`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_buffers_mb: Option<u64>,
    /// Texture upload staging buffer pool budget in MiB (default `192`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_budget_mb: Option<u64>,
    /// Presented frames retired GPU resources are held before reclamation (default `8`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_frames: Option<u64>,
    /// Ambient light fallback level (`"0.6"`, `"off"`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<String>,
    /// Whether hair strand compute/draw shaders are enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hair: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    /// The user's dump of Grand Theft Auto VI.
    pub game_dir: Option<PathBuf>,
    /// Root for all derived data. Always outside the repository.
    pub cache_dir: PathBuf,
    /// What titles write and must keep (persistent local storage). Never
    /// inside the cache, which may be deleted at any time.
    pub data_dir: PathBuf,
    /// Other titles used to exercise platform-level code.
    pub test_titles: Vec<PathBuf>,
    /// GPU, VRAM, and FSR settings.
    pub gpu: GpuConfig,
    /// The config file that was read, if any.
    pub source: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    paths: FilePaths,
    #[serde(default)]
    gpu: GpuConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePaths {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    game_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    test_titles: Vec<PathBuf>,
}

impl Config {
    /// Loads configuration from the environment and the config file.
    pub fn load() -> Result<Self> {
        let explicit = std::env::var_os("VICTORY_CONFIG").map(PathBuf::from);
        let path = explicit
            .clone()
            .unwrap_or_else(|| PathBuf::from("victory.toml"));
        let file = if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            Some((path, text))
        } else if let Some(p) = explicit {
            anyhow::bail!(
                "VICTORY_CONFIG points at {}, which does not exist",
                p.display()
            );
        } else {
            None
        };
        let (source, parsed) = match file {
            Some((path, text)) => {
                let parsed: FileConfig =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                (Some(path), parsed)
            }
            None => (None, FileConfig::default()),
        };
        Self::resolve_with_gpu(parsed.paths, parsed.gpu, source, |k| std::env::var_os(k))
    }

    #[cfg(test)]
    fn resolve(
        file: FilePaths,
        source: Option<PathBuf>,
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Self> {
        Self::resolve_with_gpu(file, GpuConfig::default(), source, env)
    }

    fn resolve_with_gpu(
        file: FilePaths,
        mut gpu: GpuConfig,
        source: Option<PathBuf>,
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Self> {
        let env_str = |k: &str| {
            env(k)
                .and_then(|v| v.into_string().ok())
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        let env_path = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let test_titles = match env("VICTORY_TEST_TITLES") {
            Some(v) if !v.is_empty() => std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect(),
            _ => file.test_titles,
        };
        let cache_dir = env_path("VICTORY_CACHE_DIR")
            .or(file.cache_dir)
            .map_or_else(|| default_dir("cache"), Ok)?;
        let data_dir = env_path("VICTORY_DATA_DIR")
            .or(file.data_dir)
            .map_or_else(|| default_dir("data"), Ok)?;
        if let Some(v) = env_str("VICTORY_FSR") {
            gpu.fsr = Some(v);
        }
        if let Some(v) = env_str("VICTORY_FSR_SHARPNESS").and_then(|s| s.parse::<f32>().ok()) {
            gpu.fsr_sharpness = Some(v);
        }
        if let Some(v) = env_str("VICTORY_FSR_RESOLUTION") {
            gpu.fsr_resolution = Some(v);
        }
        if let Some(v) = env_str("VICTORY_VRAM_BUDGET_MB").and_then(|s| s.parse::<u64>().ok()) {
            gpu.vram_budget_mb = Some(v);
        }
        if let Some(v) = env_str("VICTORY_TEXTURE_MAX").and_then(|s| s.parse::<u64>().ok()) {
            gpu.texture_max = Some(v);
        }
        if let Some(v) = env_str("VICTORY_CARD_BUFFERS_MB").and_then(|s| s.parse::<u64>().ok()) {
            gpu.card_buffers_mb = Some(v);
        }
        if let Some(v) = env_str("VICTORY_STAGING_BUDGET_MB").and_then(|s| s.parse::<u64>().ok()) {
            gpu.staging_budget_mb = Some(v);
        }
        if let Some(v) = env_str("VICTORY_KEEP_FRAMES").and_then(|s| s.parse::<u64>().ok()) {
            gpu.keep_frames = Some(v);
        }
        if let Some(v) = env_str("VICTORY_AMBIENT") {
            gpu.ambient = Some(v);
        }
        if let Some(v) = env_str("VICTORY_HAIR") {
            gpu.hair = Some(v != "0" && !v.eq_ignore_ascii_case("false"));
        }
        Ok(Config {
            game_dir: env_path("VICTORY_GAME_DIR").or(file.game_dir),
            cache_dir,
            data_dir,
            test_titles,
            gpu,
            source,
        })
    }

    /// Saves the current configuration to `path` (or `self.source` / `./victory.toml`).
    pub fn save(&self, target: Option<&Path>) -> Result<PathBuf> {
        let path = target
            .map(Path::to_path_buf)
            .or_else(|| self.source.clone())
            .or_else(|| std::env::var_os("VICTORY_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("victory.toml"));
        let file = FileConfig {
            paths: FilePaths {
                game_dir: self.game_dir.clone(),
                cache_dir: Some(self.cache_dir.clone()),
                data_dir: Some(self.data_dir.clone()),
                test_titles: self.test_titles.clone(),
            },
            gpu: self.gpu.clone(),
        };
        let text = toml::to_string_pretty(&file).context("serializing config to TOML")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Propagates `[gpu]` config fields into the process environment variables
    /// consulted by `victory-gpu` when those variables are not already set.
    pub fn apply_gpu_env(&self) {
        let set_if_unset = |key: &str, val: String| {
            if std::env::var_os(key).is_none() {
                // SAFETY: called at startup before title or worker threads spawn.
                unsafe { std::env::set_var(key, val) };
            }
        };
        if let Some(fsr) = &self.gpu.fsr {
            set_if_unset("VICTORY_FSR", fsr.clone());
        }
        if let Some(s) = self.gpu.fsr_sharpness {
            set_if_unset("VICTORY_FSR_SHARPNESS", format!("{s:.2}"));
        }
        if let Some(res) = &self.gpu.fsr_resolution {
            set_if_unset("VICTORY_FSR_RESOLUTION", res.clone());
        }
        if let Some(mb) = self.gpu.vram_budget_mb {
            set_if_unset("VICTORY_VRAM_BUDGET_MB", mb.to_string());
        }
        if let Some(max) = self.gpu.texture_max {
            set_if_unset("VICTORY_TEXTURE_MAX", max.to_string());
        }
        if let Some(mb) = self.gpu.card_buffers_mb {
            if mb == 0 {
                set_if_unset("VICTORY_CARD_BUFFERS", "0".into());
            } else {
                set_if_unset("VICTORY_CARD_BUFFERS_MB", mb.to_string());
            }
        }
        if let Some(mb) = self.gpu.staging_budget_mb {
            set_if_unset("VICTORY_STAGING_BUDGET_MB", mb.to_string());
        }
        if let Some(f) = self.gpu.keep_frames {
            set_if_unset("VICTORY_KEEP_FRAMES", f.to_string());
        }
        if let Some(a) = &self.gpu.ambient {
            set_if_unset("VICTORY_AMBIENT", a.clone());
        }
        if let Some(true) = self.gpu.hair {
            set_if_unset("VICTORY_HAIR", "1".into());
        }
    }

    /// The game directory, or a helpful error if it is not configured.
    pub fn require_game_dir(&self) -> Result<&Path> {
        self.game_dir.as_deref().context(
            "no game directory configured: set VICTORY_GAME_DIR or [paths].game_dir in \
             victory.toml (see victory.example.toml)",
        )
    }

    /// Returns `<cache>/<sub>`, creating it, after checking the asset policy.
    pub fn cache_subdir(&self, sub: &str) -> Result<PathBuf> {
        subdir(&self.cache_dir, sub)
    }

    /// Returns `<data>/<sub>`, creating it, after checking the asset policy.
    pub fn data_subdir(&self, sub: &str) -> Result<PathBuf> {
        subdir(&self.data_dir, sub)
    }
}

fn subdir(root: &Path, sub: &str) -> Result<PathBuf> {
    let dir = root.join(sub);
    ensure_outside_repo(&dir)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// `%LOCALAPPDATA%/victory/<name>` (elsewhere the XDG or home cache).
fn default_dir(name: &str) -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("cannot determine a user directory; set VICTORY_CACHE_DIR and VICTORY_DATA_DIR")?;
    Ok(base.join("victory").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn environment_overrides_file() {
        let file = FilePaths {
            game_dir: Some("from-file".into()),
            cache_dir: Some("cache-from-file".into()),
            ..Default::default()
        };
        let env = |k: &str| (k == "VICTORY_GAME_DIR").then(|| OsString::from("from-env"));
        let cfg = Config::resolve(file, None, env).unwrap();
        assert_eq!(cfg.game_dir.unwrap(), PathBuf::from("from-env"));
        assert_eq!(cfg.cache_dir, PathBuf::from("cache-from-file"));
    }

    #[test]
    fn empty_env_values_are_ignored() {
        let file = FilePaths {
            game_dir: Some("from-file".into()),
            ..Default::default()
        };
        let env = |k: &str| (k == "VICTORY_GAME_DIR").then(OsString::new);
        let cfg = Config::resolve(file, None, env).unwrap();
        assert_eq!(cfg.game_dir.unwrap(), PathBuf::from("from-file"));
    }

    #[test]
    fn empty_test_titles_env_is_ignored() {
        let file = FilePaths {
            test_titles: vec!["from-file".into()],
            ..Default::default()
        };
        let env = |k: &str| (k == "VICTORY_TEST_TITLES").then(OsString::new);
        let cfg = Config::resolve(file, None, env).unwrap();
        assert_eq!(cfg.test_titles, vec![PathBuf::from("from-file")]);
    }

    #[test]
    fn test_titles_env_splits_paths() {
        let value = std::env::join_paths(["first", "second"]).unwrap();
        let file = FilePaths::default();
        let env = |k: &str| (k == "VICTORY_TEST_TITLES").then(|| value.clone());
        let cfg = Config::resolve(file, None, env).unwrap();
        assert_eq!(
            cfg.test_titles,
            vec![PathBuf::from("first"), PathBuf::from("second")]
        );
    }

    #[test]
    fn example_config_parses() {
        let text = include_str!("../../../victory.example.toml");
        let parsed: FileConfig = toml::from_str(text).unwrap();
        assert!(parsed.paths.game_dir.is_some());
    }

    #[test]
    fn cache_inside_repo_is_refused() {
        let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
        let cfg = Config {
            cache_dir: inside,
            ..Default::default()
        };
        assert!(cfg.cache_subdir("x").is_err());
    }

    #[test]
    fn data_inside_repo_is_refused() {
        let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
        let cfg = Config {
            data_dir: inside,
            ..Default::default()
        };
        assert!(cfg.data_subdir("x").is_err());
    }
}
