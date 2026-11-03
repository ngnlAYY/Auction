//! 角色启动时冻结的排空预算；不被 common 或 Admin runtime 覆盖。
use super::{ConfigError, Role};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ShutdownFileConfig {
    drain_timeout_seconds: u64,
    critical_write_timeout_seconds: Option<u64>,
}
impl Default for ShutdownFileConfig {
    fn default() -> Self {
        Self { drain_timeout_seconds: 10, critical_write_timeout_seconds: None }
    }
}
/// 已校验的当前角色预算；关键段为零时完整选择旧排空路径。
#[derive(Debug, Clone, Copy)]
pub struct ShutdownSettings {
    pub drain_timeout_seconds: u64,
    pub critical_write_timeout_seconds: u64,
}
impl ShutdownFileConfig {
    pub(super) fn resolve(self, role: Role) -> Result<ShutdownSettings, ConfigError> {
        let critical = self.critical_write_timeout_seconds.unwrap_or(if role == Role::Worker {
            60
        } else {
            0
        });
        if !(1..=300).contains(&self.drain_timeout_seconds)
            || critical > 300
            || (role != Role::Worker && critical != 0)
        {
            return Err(ConfigError::InvalidShutdownConfig);
        }
        Ok(ShutdownSettings {
            drain_timeout_seconds: self.drain_timeout_seconds,
            critical_write_timeout_seconds: critical,
        })
    }
}
