//! Unix 部署控制器的内部状态模型。

use std::{fs::File, path::PathBuf};

use super::super::probe::ReadinessProbe;
use super::super::systemd::{ManagedRole, SystemdClient};
use auction_config::AppConfig;

pub(in crate::bootstrap::deployment_control::unix) struct Controller {
    pub(in crate::bootstrap::deployment_control::unix) state_dir: PathBuf,
    pub(in crate::bootstrap::deployment_control::unix) systemd: SystemdClient,
}

pub(in crate::bootstrap::deployment_control::unix) struct DeploymentLock {
    pub(in crate::bootstrap::deployment_control::unix) file: File,
}

impl Drop for DeploymentLock {
    fn drop(&mut self) {
        // 文件描述符关闭也会释放锁；显式 unlock 让失败路径的所有权更容易审计。
        let _ = self.file.unlock();
    }
}

pub(in crate::bootstrap::deployment_control::unix) struct PreparedDeployment {
    pub(in crate::bootstrap::deployment_control::unix) migration_config: AppConfig,
    pub(in crate::bootstrap::deployment_control::unix) plans: Vec<RolePlan>,
}

pub(in crate::bootstrap::deployment_control::unix) struct RolePlan {
    pub(in crate::bootstrap::deployment_control::unix) role: ManagedRole,
    pub(in crate::bootstrap::deployment_control::unix) probe: ReadinessProbe,
}

pub(in crate::bootstrap::deployment_control::unix) struct StartFailure {
    pub(in crate::bootstrap::deployment_control::unix) code: &'static str,
    pub(in crate::bootstrap::deployment_control::unix) error: anyhow::Error,
}

impl StartFailure {
    pub(in crate::bootstrap::deployment_control::unix) fn new(
        code: &'static str,
        error: impl Into<anyhow::Error>,
    ) -> Self {
        Self { code, error: error.into() }
    }
}
