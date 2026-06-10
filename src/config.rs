use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum PlanMode {
    Args,
    Inline,
    None,
}

impl Default for PlanMode {
    fn default() -> Self {
        PlanMode::None
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentProfile {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub plan_mode: PlanMode,
    #[serde(default)]
    pub plan_args: Vec<String>,
    #[serde(default)]
    pub plan_prefix: Option<String>,
    /// Regex pattern — compiled at runtime
    #[serde(default)]
    pub plan_trigger: Option<String>,
    #[serde(default)]
    pub plan_accept: String,
    #[serde(default)]
    pub plan_reject: String,
    /// 0 = disabled
    #[serde(default)]
    pub input_timeout_secs: u32,
    #[serde(default)]
    pub post_run_script: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    agents: Vec<AgentProfile>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub agents: Vec<AgentProfile>,
}

impl Config {
    pub fn find_agent(&self, name: &str) -> Option<&AgentProfile> {
        self.agents.iter().find(|a| a.name == name)
    }
}

pub fn load(path: impl AsRef<Path>) -> Result<Config> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read agents config: {}", path.display()))?;

    let raw: RawConfig = toml::from_str(&content)
        .with_context(|| format!("Invalid TOML in {}", path.display()))?;

    if raw.agents.is_empty() {
        bail!(
            "No agents defined in {}. Add at least one [[agents]] section.",
            path.display()
        );
    }

    // Validate regex patterns early so we fail at startup, not at runtime
    for agent in &raw.agents {
        if let Some(pattern) = &agent.plan_trigger {
            if !pattern.is_empty() {
                regex::Regex::new(pattern).with_context(|| {
                    format!(
                        "Invalid plan_trigger regex for agent '{}': {}",
                        agent.name, pattern
                    )
                })?;
            }
        }
    }

    Ok(Config { agents: raw.agents })
}
