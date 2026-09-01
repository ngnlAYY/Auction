//! Public 与 Admin session 用例：登录、滑动刷新和撤销。

use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::{
    AdminLoginResult, AdminSessionConfig, AdminSessionIdentity, AdminSessionPort,
    AdminSessionRecord, AdminSessionStore, ApplicationError, ApplicationFuture,
    PublicLoginAuthenticator, PublicLoginIdentity, PublicLoginResult, PublicSessionConfig,
    PublicSessionIdentity, PublicSessionPort, PublicSessionRecord, PublicSessionStore,
    SessionError,
};

/// Public session 应用服务。
///
/// 原始 token 只在 `login` 的返回值中出现；存储端只接收 SHA-256 摘要。会话真值和并发上限
/// 由 sessions owner 持久化，缓存不能替代该端口。
pub struct PublicSessionService {
    authenticator: Arc<dyn PublicLoginAuthenticator>,
    store: Arc<dyn PublicSessionStore>,
    config: PublicSessionConfig,
}

impl PublicSessionService {
    /// 创建并校验 Public session 用例服务。
    pub fn new(
        authenticator: Arc<dyn PublicLoginAuthenticator>,
        store: Arc<dyn PublicSessionStore>,
        config: PublicSessionConfig,
    ) -> Result<Self, ApplicationError> {
        config.validate()?;
        Ok(Self { authenticator, store, config })
    }
}

impl PublicSessionPort for PublicSessionService {
    fn login(&self, key: Vec<u8>, now: DateTime<Utc>) -> ApplicationFuture<'_, PublicLoginResult> {
        Box::pin(async move {
            validate_login_key(&key)?;
            let login = self.authenticator.authenticate(key).await?;
            validate_login_identity(&login)?;
            if !login.active {
                return Err(
                    SessionError::AccountInactive { public_reason: login.public_reason }.into()
                );
            }
            if login.public_reason.is_some() {
                return Err(SessionError::InvalidIdentity.into());
            }

            let (token, token_hash) = generate_token()?;
            let expires_at = now
                .checked_add_signed(self.config.idle_ttl)
                .ok_or(SessionError::InvalidConfiguration)?;
            let absolute_expires_at = now
                .checked_add_signed(self.config.absolute_ttl)
                .ok_or(SessionError::InvalidConfiguration)?;
            let stored = self
                .store
                .create_public(
                    PublicSessionRecord {
                        token_hash,
                        user_id: login.user_id,
                        created_at: now,
                        expires_at,
                        absolute_expires_at,
                    },
                    self.config.max_public_sessions,
                )
                .await?;
            if stored.user_id != login.user_id
                || stored.username != login.username
                || stored.game_player_id != login.game_player_id
            {
                return Err(SessionError::InvalidIdentity.into());
            }
            Ok(PublicLoginResult { token, identity: stored })
        })
    }

    fn current(
        &self,
        raw_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store
                .touch_public(
                    token_hash,
                    now,
                    now.checked_add_signed(self.config.idle_ttl)
                        .ok_or(SessionError::InvalidConfiguration)?,
                )
                .await
        })
    }

    fn logout(&self, raw_token: String, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.revoke_public(token_hash, now).await
        })
    }
}

/// Admin session 应用服务。
///
/// 管理凭据只在装配时输入并立即转换为摘要；HTTP 只提交 Basic Auth 字段，持久化层只接收
/// session/CSRF 摘要。Admin 会话不复用 Public 用户会话，也不依赖缓存。
pub struct AdminSessionService {
    store: Arc<dyn AdminSessionStore>,
    expected_actor: String,
    secret_hash: [u8; 32],
    config: AdminSessionConfig,
}

impl AdminSessionService {
    /// 创建并校验 Admin session 用例服务；原始登录 secret 不会保存在服务对象中。
    pub fn new(
        store: Arc<dyn AdminSessionStore>,
        actor: String,
        secret: Vec<u8>,
        config: AdminSessionConfig,
    ) -> Result<Self, ApplicationError> {
        config.validate()?;
        validate_admin_actor(&actor)?;
        validate_admin_secret(&secret)?;
        Ok(Self {
            store,
            expected_actor: actor,
            secret_hash: Sha256::digest(secret).into(),
            config,
        })
    }
}

impl AdminSessionPort for AdminSessionService {
    fn login(
        &self,
        actor: String,
        secret: Vec<u8>,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminLoginResult> {
        Box::pin(async move {
            validate_admin_actor(&actor)?;
            if !is_valid_admin_secret(&secret) {
                return Err(SessionError::InvalidCredentials.into());
            }
            let presented_hash = Sha256::digest(&secret);
            if actor != self.expected_actor
                || presented_hash.as_slice().ct_eq(&self.secret_hash).unwrap_u8() != 1
            {
                return Err(SessionError::InvalidCredentials.into());
            }

            let (token, token_hash) = generate_token()?;
            let (csrf_token, csrf_hash) = generate_token()?;
            let expires_at = now
                .checked_add_signed(self.config.idle_ttl)
                .ok_or(SessionError::InvalidConfiguration)?;
            let absolute_expires_at = now
                .checked_add_signed(self.config.absolute_ttl)
                .ok_or(SessionError::InvalidConfiguration)?;
            let identity = self
                .store
                .create_admin(AdminSessionRecord {
                    token_hash,
                    csrf_hash,
                    actor: actor.clone(),
                    created_at: now,
                    expires_at,
                    absolute_expires_at,
                })
                .await?;
            if identity.actor != actor
                || identity.expires_at != expires_at
                || identity.absolute_expires_at != absolute_expires_at
            {
                return Err(SessionError::InvalidIdentity.into());
            }
            Ok(AdminLoginResult { token, csrf_token, identity })
        })
    }

    fn current(
        &self,
        raw_token: String,
        raw_csrf_token: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            let csrf_hash = hash_token(&raw_csrf_token)?;
            self.store
                .touch_admin(
                    token_hash,
                    csrf_hash,
                    now,
                    now.checked_add_signed(self.config.idle_ttl)
                        .ok_or(SessionError::InvalidConfiguration)?,
                )
                .await
        })
    }

    fn logout(&self, raw_token: String, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.revoke_admin(token_hash, now).await
        })
    }
}

fn validate_login_key(key: &[u8]) -> Result<(), ApplicationError> {
    let value = std::str::from_utf8(key).map_err(|_| SessionError::InvalidLoginKey)?;
    if value.is_empty() || value.len() > 1024 || value != value.trim() {
        return Err(SessionError::InvalidLoginKey.into());
    }
    Ok(())
}

fn validate_admin_actor(actor: &str) -> Result<(), ApplicationError> {
    if actor.is_empty()
        || actor.len() > 256
        || actor != actor.trim()
        || actor.chars().any(char::is_control)
    {
        return Err(SessionError::InvalidCredentials.into());
    }
    Ok(())
}

fn validate_admin_secret(secret: &[u8]) -> Result<(), ApplicationError> {
    if !is_valid_admin_secret(secret) {
        return Err(SessionError::InvalidConfiguration.into());
    }
    Ok(())
}

fn is_valid_admin_secret(secret: &[u8]) -> bool {
    (16..=4096).contains(&secret.len()) && !secret.iter().any(u8::is_ascii_control)
}

fn validate_login_identity(identity: &PublicLoginIdentity) -> Result<(), ApplicationError> {
    if identity.user_id.is_nil()
        || identity.username.is_empty()
        || identity.username.len() > 256
        || identity.username != identity.username.trim()
        || identity.username.chars().any(char::is_control)
        || identity.game_player_id.as_deref().is_some_and(|value| {
            value.is_empty()
                || value.len() > 256
                || value != value.trim()
                || value.chars().any(char::is_control)
        })
    {
        return Err(SessionError::InvalidIdentity.into());
    }
    if !identity.active {
        let Some(reason) = identity.public_reason.as_deref() else {
            return Err(SessionError::InvalidCredentials.into());
        };
        if reason.is_empty()
            || reason.len() > 1024
            || reason != reason.trim()
            || reason.chars().any(char::is_control)
        {
            return Err(SessionError::InvalidCredentials.into());
        }
    }
    Ok(())
}

fn generate_token() -> Result<(String, [u8; 32]), ApplicationError> {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    let token = URL_SAFE_NO_PAD.encode(bytes);
    Ok((token, Sha256::digest(bytes).into()))
}

fn hash_token(raw_token: &str) -> Result<[u8; 32], ApplicationError> {
    let decoded = URL_SAFE_NO_PAD.decode(raw_token).map_err(|_| SessionError::Unauthenticated)?;
    if decoded.len() != 32 {
        return Err(SessionError::Unauthenticated.into());
    }
    Ok(Sha256::digest(decoded).into())
}
