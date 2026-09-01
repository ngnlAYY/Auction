use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::ApplicationFuture;
use crate::SessionError;

/// Public 会话的滑动过期、绝对过期和并发上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicSessionConfig {
    pub idle_ttl: Duration,
    pub absolute_ttl: Duration,
    pub max_public_sessions: u32,
}

impl PublicSessionConfig {
    /// 校验会话时间必须为正，且滑动过期不能超过绝对过期。
    pub fn validate(self) -> Result<(), SessionError> {
        if self.idle_ttl <= Duration::zero()
            || self.absolute_ttl < self.idle_ttl
            || self.max_public_sessions == 0
        {
            return Err(SessionError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Admin 会话的滑动过期和绝对过期配置。
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
    pub username: String,
    pub game_player_id: Option<String>,
    pub active: bool,
    pub public_reason: Option<String>,
}

/// 成功认证并刷新后的公开会话投影。
#[derive(Debug, Clone)]
pub struct PublicSessionIdentity {
    pub user_id: Uuid,
    pub username: String,
    pub game_player_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// sessions owner 接收的公开会话持久化记录。
#[derive(Debug, Clone)]
pub struct PublicSessionRecord {
    pub token_hash: [u8; 32],
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// 登录成功后只向 HTTP 边界暴露一次的原始 token 和安全投影。
#[derive(Debug, Clone)]
pub struct PublicLoginResult {
    pub token: String,
    pub identity: PublicSessionIdentity,
}

/// 成功认证并刷新后的 Admin 会话投影。
#[derive(Debug, Clone)]
pub struct AdminSessionIdentity {
    pub actor: String,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// sessions owner 接收的 Admin 会话和 CSRF 摘要记录。
#[derive(Debug, Clone)]
pub struct AdminSessionRecord {
    pub token_hash: [u8; 32],
    pub csrf_hash: [u8; 32],
    pub actor: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
}

/// Admin 登录成功后只向 HTTP 边界暴露一次的原始 token 对。
#[derive(Debug, Clone)]
pub struct AdminLoginResult {
    pub token: String,
    pub csrf_token: String,
    pub identity: AdminSessionIdentity,
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

    fn touch_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    fn revoke_public(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}

/// Admin 会话持久化窄端口；数据库只保存 token 和 CSRF 摘要。
pub trait AdminSessionStore: Send + Sync {
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
    fn login(&self, key: Vec<u8>, now: DateTime<Utc>) -> ApplicationFuture<'_, PublicLoginResult>;

    fn current(
        &self,
        raw_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity>;

    fn logout(&self, raw_token: String, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
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
