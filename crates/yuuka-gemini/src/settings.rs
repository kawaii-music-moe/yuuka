//! Provider/model selection shared by API settings and generation.
use std::collections::HashMap;
use std::sync::OnceLock;

use yuuka_core::ConfigError;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmSettings {
    pub provider: String,
    pub model: String,
}

static DEFAULTS: OnceLock<LlmSettings> = OnceLock::new();

impl Default for LlmSettings {
    fn default() -> Self {
        DEFAULTS.get().cloned().unwrap_or_else(|| Self {
            provider: "gemini".to_owned(),
            model: crate::DEFAULT_MODEL.to_owned(),
        })
    }
}

impl LlmSettings {
    /// Validate before persisting; unsupported providers never silently use Gemini.
    pub fn validate(provider: &str, model: &str) -> Result<Self, ConfigError> {
        if provider != "gemini" {
            return Err(ConfigError::InvalidValue {
                field: "LLM_PROVIDER",
                reason: "対応プロバイダは gemini のみです。".to_owned(),
            });
        }
        if !crate::ALLOWED_MODELS.contains(&model) {
            return Err(ConfigError::InvalidValue {
                field: "LLM_MODEL",
                reason: "対応する Gemini モデルを選択してください。".to_owned(),
            });
        }
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
        })
    }
}

/// Read .env without modifying the process environment. Exported values take precedence.
/// Called once at startup, before services are started.
pub fn init_llm_defaults() -> Result<(), ConfigError> {
    let defaults = load_defaults(std::path::Path::new(".env"), |key| std::env::var(key).ok())?;
    tracing::info!(provider = %defaults.provider, model = %defaults.model, "LLM defaults loaded");
    let _ = DEFAULTS.set(defaults);
    Ok(())
}

fn load_defaults(
    path: &std::path::Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<LlmSettings, ConfigError> {
    let mut values = HashMap::new();
    match dotenvy::from_path_iter(path) {
        Ok(iter) => {
            for entry in iter {
                let (key, value) = entry.map_err(|_| ConfigError::InvalidValue {
                    field: ".env",
                    reason: "環境変数ファイルの形式が不正です。".to_owned(),
                })?;
                values.insert(key, value);
            }
        }
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ConfigError::InvalidValue {
                field: ".env",
                reason: "環境変数ファイルを読み込めません。".to_owned(),
            })
        }
    }
    let get = |key: &str, fallback: &str| {
        lookup(key)
            .or_else(|| values.get(key).cloned())
            .unwrap_or_else(|| fallback.to_owned())
    };
    LlmSettings::validate(
        get("LLM_PROVIDER", "gemini").trim(),
        get("LLM_MODEL", crate::DEFAULT_MODEL).trim(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn dotenv_defaults_and_exported_environment_precedence() {
        let path =
            std::env::temp_dir().join(format!("yuuka-llm-defaults-{}.env", std::process::id()));
        std::fs::write(
            &path,
            "LLM_PROVIDER=gemini\nLLM_MODEL=gemini-3.5-flash-lite\n",
        )
        .unwrap();
        assert_eq!(
            load_defaults(&path, |_| None).unwrap().model,
            "gemini-3.5-flash-lite"
        );
        assert_eq!(
            load_defaults(&path, |key| (key == "LLM_MODEL")
                .then(|| "gemini-2.5-flash".to_owned()))
            .unwrap()
            .model,
            "gemini-2.5-flash"
        );
        assert!(load_defaults(&path, |key| (key == "LLM_PROVIDER")
            .then(|| "openai".to_owned()))
        .is_err());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            load_defaults(&path, |_| None).unwrap().model,
            crate::DEFAULT_MODEL
        );
    }
    #[test]
    fn rejects_unsupported_provider_and_model() {
        assert!(LlmSettings::validate("openai", "gemini-3.5-flash-lite").is_err());
        assert!(LlmSettings::validate("gemini", "gpt-4o").is_err());
        assert!(LlmSettings::validate("gemini", "").is_err());
        assert!(LlmSettings::validate("gemini", "gemini-3.5-flash-lite").is_ok());
    }
}
