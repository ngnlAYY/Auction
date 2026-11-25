//! 页面静态挂载根：Public 与 Admin 从中加载挂载页面，常驻角色与 CLI 从中读取页面栈回执。
//!
//! 只做形状校验，不访问文件系统；挂载内容与回执在装配根启动时一次性读取并核对（E1）。

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::settings::errors::ConfigError;

/// 页面栈回执在挂载根下的固定文件名，由 `auction-cli web install / switch` 写出。
pub const WEB_STACK_RECEIPT_FILE: &str = "RECEIPT.v1";

/// 已校验的页面挂载配置；三角色从同一个公共文件读取。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebConfig {
    mount_root: Option<PathBuf>,
}

impl WebConfig {
    /// 静态挂载根；省略时页面只用内嵌产物，响应与未支持挂载的程序一致。
    pub fn mount_root(&self) -> Option<&Path> {
        self.mount_root.as_deref()
    }

    /// 挂载根下的页面栈回执路径；没有挂载根时没有回执，开启寄售会失败关闭。
    pub fn receipt_path(&self) -> Option<PathBuf> {
        self.mount_root.as_deref().map(|root| root.join(WEB_STACK_RECEIPT_FILE))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct WebFileConfig {
    mount_root: Option<PathBuf>,
}

impl WebFileConfig {
    pub(super) fn resolve(self) -> Result<WebConfig, ConfigError> {
        // 挂载根拼出 surface、build 与回执路径；只接受不含 `..` 的绝对路径，避免随工作目录或链接漂移。
        if let Some(root) = self.mount_root.as_deref()
            && !is_canonical_absolute(root)
        {
            return Err(ConfigError::InvalidWebMountRoot);
        }
        Ok(WebConfig { mount_root: self.mount_root })
    }
}

fn is_canonical_absolute(path: &Path) -> bool {
    let mut components = path.components();
    components.next() == Some(Component::RootDir)
        && path.components().count() > 1
        && components.all(|component| matches!(component, Component::Normal(_)))
}
