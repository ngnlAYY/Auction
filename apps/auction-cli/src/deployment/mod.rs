//! 三角色配置目录、单角色安全投影与部署一致性预检；只读文件，不连接数据库或启动进程。

use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use auction_config::{AppConfig, Role};

mod budget;
mod check;
mod listener;
pub(crate) mod receipt;
mod summary;

pub(crate) use check::{Deployment, check};
pub(crate) use listener::Endpoint;
pub(crate) use summary::show;

/// 已规范化的绝对配置目录；分体与合并部署共用 `<dir>/<role>.toml` 布局。
#[derive(Debug, Clone)]
pub(crate) struct ConfigDir {
    path: PathBuf,
}

impl ConfigDir {
    /// 与 auction-server 相同：规范化父目录、保留目录名，使合并部署运行目录推导一致。
    pub(crate) fn new(input: &Path) -> anyhow::Result<Self> {
        let absolute = std::path::absolute(input).context("解析配置目录路径失败")?;
        if absolute.components().any(|part| matches!(part, Component::ParentDir)) {
            anyhow::bail!("配置目录不得包含 `..`：{}", absolute.display());
        }
        let name = absolute.file_name().context("配置目录必须有目录名")?;
        let path = match absolute.parent() {
            Some(parent) => match parent.canonicalize() {
                Ok(parent) => parent.join(name),
                // 父目录缺失时保留原路径，由随后的读取给出明确的不存在错误。
                Err(_) => absolute.clone(),
            },
            None => absolute.clone(),
        };
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn role_path(&self, role: Role) -> PathBuf {
        self.path.join(format!("{}.toml", role.name()))
    }

    /// 加载单个角色；校验与默认值全部由配置 crate 负责。
    pub(crate) fn load(&self, role: Role) -> anyhow::Result<AppConfig> {
        let path = self.role_path(role);
        AppConfig::load_from_path(role, &path)
            .with_context(|| format!("加载 {} 配置失败：{}", role.name(), path.display()))
    }

    /// 数据库、凭据与邮件运维统一读取 Worker 配置；三角色共享目标由 `config check` 保证。
    pub(crate) fn load_worker(&self) -> anyhow::Result<AppConfig> {
        self.load(Role::Worker)
    }
}
