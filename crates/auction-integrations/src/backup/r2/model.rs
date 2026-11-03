//! 对外暴露的副本证据和脱敏错误闭集；不保留 SDK 错误链、签名 URL 或远端正文。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 本次上传或回读确认已有相同对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupObjectStatus {
    /// 上传成功，且已完成 HEAD 和完整回读。
    Uploaded,
    /// 对象原已存在，或上传响应未知后回读确认内容相同。
    AlreadyPresent,
}

/// 已通过长度和完整 SHA-256 校验的单个远端对象。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupObject {
    /// 无凭据的 bucket 内对象名。
    pub key: String,
    /// 密文或完成记录的字节数。
    pub bytes: u64,
    /// 整个对象的 SHA-256；不使用 multipart ETag 代替摘要。
    pub sha256: String,
    /// 上传或确认已有对象。
    pub status: BackupObjectStatus,
}

/// 固定分类，可直接用于脱敏日志；任何分支均不包含底层错误的 Display。
#[derive(Debug, Clone, Copy, Error)]
pub enum BackupStoreError {
    /// 输入或客户端构造失败。
    #[error("offsite_configuration_invalid")]
    Configuration,
    /// 本地密文读取失败。
    #[error("offsite_local_read_failed")]
    LocalRead,
    /// 认证或 bucket 权限不足。
    #[error("offsite_access_denied")]
    AccessDenied,
    /// 请求超时或远端错误，尚无相同对象的完成证据。
    #[error("offsite_transport_failed")]
    Transport,
    /// 远端已有对象与原密文长度或摘要不同；禁止覆盖、删除或换名。
    #[error("offsite_object_conflict")]
    Conflict,
    /// 远端完成记录缺件、过大或内容无效。
    #[error("offsite_remote_record_invalid")]
    RemoteRecord,
}

impl BackupStoreError {
    pub(super) fn from_store(error: object_store::Error) -> Self {
        match error {
            object_store::Error::PermissionDenied { .. }
            | object_store::Error::Unauthenticated { .. } => Self::AccessDenied,
            object_store::Error::InvalidPath { .. } => Self::Configuration,
            _ => Self::Transport,
        }
    }
}
