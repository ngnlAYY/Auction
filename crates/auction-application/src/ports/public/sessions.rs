//! Public 固定期限/永久会话与 Admin 滑动会话的应用端口。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApplicationFuture;
use crate::{PublicSessionPreferenceResult, SessionError, SetPublicSessionPreferenceCommand};

/// Public 会话的并发上限；登录期限由账号偏好决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicSessionConfig {
    pub max_public_sessions: u32,
}

impl PublicSessionConfig {
    /// 并发上限必须为正，不再以部署配置控制 Public 登录期限。
    pub fn validate(self) -> Result<(), SessionError> {
        if self.max_public_sessions == 0 {
            return Err(SessionError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminSessionConfig {
    pub idle_ttl: Duration,
    pub absolute_ttl: Duration,
}

impl AdminSessionConfig {
    /// 校验管理会话的过期边界。
    pub fn validate(self) -> Result<(), SessionError> {
        if self.idle_ttl <= Duration::zero() || self.absolute_ttl < self.idle_ttl {
            return Err(SessionError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// users owner 暴露给 Public session 的最小登录投影。
#[derive(Debug, Clone)]
pub struct PublicLoginIdentity {
    pub user_id: Uuid,
    /// 认证时读取的用户版本；创建会话时必须在写事务内复核，阻止凭据轮换后的旧认证结果。
    pub user_state_version: i64,
    pub username: String,
    pub active: bool,
    pub public_reason: Option<String>,
}

/// 公开会话的安全投影；双截止同为空表示永久，读取或活动不延长期限。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSessionIdentity {
    pub user_id: Uuid,
    pub username: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub absolute_expires_at: Option<DateTime<Utc>>,
}

impl PublicSessionIdentity {
    /// 旧会话保留双截止的较早值；永久没有截止，混合空值保守采用存在的截止。
    pub fn effective_expires_at(&self) -> Option<DateTime<Utc>> {
        match (self.expires_at, self.absolute_expires_at) {
            (Some(expires_at), Some(absolute_expires_at)) => {
                Some(expires_at.min(absolute_expires_at))
            }
            (expires_at, absolute_expires_at) => expires_at.or(absolute_expires_at),
        }
    }
}

/// sessions owner 接收的登录记录；存储锁定用户后读取偏好并计算固定截止。
#[derive(Debug, Clone, Copy)]
pub struct PublicSessionRecord {
    pub token_hash: [u8; 32],
    pub user_id: Uuid,
    /// 仅用于创建会话的版本门禁，不持久化凭据摘要或改变既有会话的响应契约。
    pub authenticated_user_version: i64,
    pub created_at: DateTime<Utc>,
}

/// 登录成功后只向 HTTP 边界暴露一次的原始 token 和安全投影；调试输出不包含 token。
#[derive(Clone)]
pub struct PublicLoginResult {
    pub token: String,
    pub identity: PublicSessionIdentity,
}

impl std::fmt::Debug for PublicLoginResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLoginResult")
            .field("token", &"[REDACTED]")
            .field("identity", &self.identity)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct AdminSessionIdentity {
    pub actor: String,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// sessions owner 接收的 Admin 会话和 CSRF 摘要记录；token 摘要已绑定当前管理凭据。
#[derive(Debug, Clone)]
pub struct AdminSessionRecord {
    pub token_hash: [u8; 32],
    pub csrf_hash: [u8; 32],
    pub actor: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// Admin 登录成功后只向 HTTP 边界暴露一次的原始 token 对；调试输出同时隐藏 session 与 CSRF。
#[derive(Clone)]
pub struct AdminLoginResult {
    pub token: String,
    pub csrf_token: String,
    pub identity: AdminSessionIdentity,
}

impl std::fmt::Debug for AdminLoginResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminLoginResult")
            .field("token", &"[REDACTED]")
            .field("csrf_token", &"[REDACTED]")
            .field("identity", &self.identity)
            .finish()
    }
}

/// users owner 的登录认证窄端口。
pub trait PublicLoginAuthenticator: Send + Sync {
    fn authenticate(&self, key: Vec<u8>) -> ApplicationFuture<'_, PublicLoginIdentity>;
}

/// sessions owner 的公开会话持久化窄端口。
pub trait PublicSessionStore: Send + Sync {
    fn create_public(
        &self,
        record: PublicSessionRecord,
        max_public_sessions: u32,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    /// 只读复核撤销、账号/凭据状态和双重过期边界，不刷新活动时间。
    fn inspect_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    /// 复核会话并节流记录最后活动时间（距上次记录满 60 秒才写入），不延长旧截止或永久会话。
    fn touch_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    fn revoke_public(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;

    /// 同一查询中读取有效会话和账号偏好，不刷新活动时间或期限。
    fn get_public_preference(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult>;

    /// 同事务锁用户后锁当前会话，复核授权，再重放或原子保存偏好、截止和账本。
    fn set_public_preference(
        &self,
        token_hash: [u8; 32],
        command: SetPublicSessionPreferenceCommand,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult>;
}

/// Admin 会话持久化窄端口；数据库只保存 token 和 CSRF 摘要。
pub trait AdminSessionStore: Send + Sync {
    /// Admin 启动前持久撤销全部旧会话；重复执行不改写既有撤销时间，也不触及 Public。
    fn revoke_all_admin(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;

    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity>;

    fn touch_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity>;

    fn revoke_admin(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}

/// Public session 用例端口，供 HTTP 或其他交付表面使用。
pub trait PublicSessionPort: Send + Sync {
    /// 长连接的只读认证复核；心跳不能替代用户活动或延长会话。
    fn inspect(
        &self,
        raw_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    fn login(&self, key: Vec<u8>, now: DateTime<Utc>) -> ApplicationFuture<'_, PublicLoginResult>;

    fn current(
        &self,
        raw_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    fn logout(&self, raw_token: String, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;

    /// 返回账号默认时长和当前会话实际截止；二者可以因旧会话或其他设备而不同。
    fn get_preference(
        &self,
        raw_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult>;

    /// 保存后续登录默认值，并且只重设本次认证会话的固定截止。
    fn set_preference(
        &self,
        raw_token: String,
        command: SetPublicSessionPreferenceCommand,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult>;
}

/// Admin session 用例端口，供管理 HTTP 表面使用。
pub trait AdminSessionPort: Send + Sync {
    fn login(
        &self,
        actor: String,
        secret: Vec<u8>,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminLoginResult>;

    fn current(
        &self,
        raw_token: String,
        raw_csrf_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity>;

    fn logout(&self, raw_token: String, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}
