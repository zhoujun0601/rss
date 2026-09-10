use std::{env, fs, path::Path};

use anyhow::{Context, Result, bail};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
#[allow(non_snake_case)]
pub struct Config {
    pub BotToken: String,
    pub ADMINIDS: i64,
    pub Cycletime: u64,
    pub Debug: bool,
    pub ProxyURL: String,
    pub Pushinfo: String,
    pub TZ: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            BotToken: String::new(),
            ADMINIDS: 0,
            Cycletime: 300,
            Debug: false,
            ProxyURL: String::new(),
            Pushinfo: String::new(),
            TZ: "Asia/Shanghai".to_owned(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        let mut config: Self = serde_json::from_str(&raw)
            .with_context(|| format!("无法解析配置文件 {}", path.display()))?;
        config.apply_environment()?;
        config.validate()?;
        Ok(config)
    }

    fn apply_environment(&mut self) -> Result<()> {
        if let Some(value) = env_value("BotToken") {
            self.BotToken = value;
        }
        if let Some(value) = env_value("ADMINIDS") {
            self.ADMINIDS = value.parse().context("环境变量 ADMINIDS 必须是整数")?;
        }
        if let Some(value) = env_value("Cycletime") {
            let cycle: i64 = value.parse().context("环境变量 Cycletime 必须是整数")?;
            if cycle <= 0 {
                bail!("Cycletime 必须为正整数");
            }
            self.Cycletime = cycle as u64;
        }
        if let Some(value) = env_value("Debug") {
            self.Debug = value
                .parse()
                .context("环境变量 Debug 必须是 true 或 false")?;
        }
        if let Some(value) = env_value("ProxyURL") {
            self.ProxyURL = value;
        }
        if let Some(value) = env_value("Pushinfo") {
            self.Pushinfo = value;
        }
        if let Some(value) = env_value("TZ") {
            self.TZ = value;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.BotToken.trim().is_empty()
            || self.BotToken == "YOUR_BOT_TOKEN_HERE"
            || self.BotToken == "your_bot_token_here"
        {
            bail!("BotToken 不能为空或使用模板占位值");
        }
        if self.Cycletime == 0 {
            bail!("Cycletime 必须为正整数");
        }
        if !self.ProxyURL.trim().is_empty() {
            validate_http_url(&self.ProxyURL).context("ProxyURL 无效")?;
        }
        if !self.Pushinfo.trim().is_empty() {
            validate_http_url(&self.Pushinfo).context("Pushinfo 无效")?;
        }
        self.TZ
            .parse::<Tz>()
            .with_context(|| format!("TZ 不是有效的 IANA 时区: {}", self.TZ))?;
        Ok(())
    }

    pub fn timezone(&self) -> Tz {
        self.TZ.parse().unwrap_or(chrono_tz::Asia::Shanghai)
    }

    pub fn is_authorized(&self, user_id: i64) -> bool {
        self.ADMINIDS == 0 || self.ADMINIDS == user_id
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var_os(name).map(|value| value.to_string_lossy().into_owned())
}

fn validate_http_url(raw: &str) -> Result<()> {
    let parsed = Url::parse(raw.trim())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("只支持带主机名的 http/https URL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_placeholder_token() {
        let config = Config {
            BotToken: "YOUR_BOT_TOKEN_HERE".into(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_timezone_and_cycle() {
        let mut config = Config {
            BotToken: "123:test".into(),
            ..Config::default()
        };
        assert!(config.validate().is_ok());
        config.Cycletime = 0;
        assert!(config.validate().is_err());
        config.Cycletime = 1;
        config.TZ = "not/a-zone".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validates_pushinfo_scheme() {
        let config = Config {
            BotToken: "123:test".into(),
            Pushinfo: "file:///tmp/push".into(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
}
