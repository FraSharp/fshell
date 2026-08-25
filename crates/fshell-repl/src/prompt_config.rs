// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::prompt_config::PromptConfig;

pub fn load_config() -> PromptConfig {
    let path = fshell_engine::resolve_config_dir().map(|p| p.join("prompt.toml"));
    if let Some(path) = path
        && path.exists()
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(cfg) = toml_edit::de::from_str(&content)
    {
        return cfg;
    }
    PromptConfig::default()
}

pub fn save_config(config: &PromptConfig) -> Result<(), String> {
    let dir = fshell_engine::ensure_config_dir()
        .ok_or_else(|| "cannot find or create config directory".to_string())?;
    let content =
        toml_edit::ser::to_string_pretty(config).map_err(|e| format!("serialization: {}", e))?;
    std::fs::write(dir.join("prompt.toml"), &content).map_err(|e| format!("write error: {}", e))?;
    Ok(())
}
