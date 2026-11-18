//! Public 固定期限/永久登录、偏好与只读复核；Admin 校验、人工操作续期与撤销。

use std::sync::Arc;

use auction_time::{Duration, Timestamp};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::errors::{ApplicationError, SessionError};
use crate::ports::core::ApplicationFuture;
use crate::ports::public::{
    AdminLoginResult, AdminSessionConfig, AdminSessionIdentity, AdminSessionPort,
    AdminSessionRecord, AdminSessionStore, PublicLoginAuthenticator, PublicLoginIdentity,
    PublicLoginResult, PublicSessionConfig, PublicSessionIdentity, PublicSessionPort,
    PublicSessionPreferenceResult, PublicSessionRecord, PublicSessionStore,
    SetPublicSessionPreferenceCommand,
};

/// Public session 应用服务。
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
    fn inspect(
        &self,
        raw_token: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.inspect_public(token_hash, now).await
        })
    }

    fn login(&self, key: Vec<u8>, now: Timestamp) -> ApplicationFuture<'_, PublicLoginResult> {
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

            let (token, token_hash) = generate_token();
            let stored = self
                .store
                .create_public(
                    PublicSessionRecord {
                        token_hash,
                        user_id: login.user_id,
                        authenticated_user_version: login.user_state_version,
                        created_at: now,
                    },
                    self.config.max_public_sessions,
                )
                .await?;
            if stored.user_id != login.user_id || stored.username != login.username {
                return Err(SessionError::InvalidIdentity.into());
            }
            Ok(PublicLoginResult { token, identity: stored })
        })
    }

    fn current(
        &self,
        raw_token: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.touch_public(token_hash, now).await
        })
    }

    fn logout(&self, raw_token: String, now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.revoke_public(token_hash, now).await
        })
    }

    fn get_preference(
        &self,
        raw_token: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            self.store.get_public_preference(token_hash, now).await
        })
    }

    fn set_preference(
        &self,
        raw_token: String,
        command: SetPublicSessionPreferenceCommand,
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move {
            let token_hash = hash_token(&raw_token)?;
            command.validate()?;
            self.store.set_public_preference(token_hash, command, now).await
        })
    }
}

/// Admin session 应用服务。
/// 管理凭据只在装配时输入并立即转换为摘要；HTTP 只提交 Basic Auth 字段，持久化层只接收
/// session/CSRF 摘要。session 摘要绑定当前 actor 和 secret；Admin 不复用 Public 会话或缓存。
pub struct AdminSessionService {
    store: Arc<dyn AdminSessionStore>,
    expected_actor: String,
    secret_hash: [u8; 32],
    token_digest: Hmac<Sha256>,
    config: AdminSessionConfig,
}

impl AdminSessionService {
    /// 创建服务前永久撤销既有 Admin 会话；失败阻断启动，重启后必须重新登录。
    /// 不能只依赖确定性凭据摘要：配置从 A 改为 B 再改回 A 时，旧 token 也不能复活。
    /// 原始 secret 和可独立校验口令的摘要均不进入持久化层。
    pub async fn new(
        store: Arc<dyn AdminSessionStore>,
        actor: String,
        secret: Vec<u8>,
        config: AdminSessionConfig,
        now: Timestamp,
    ) -> Result<Self, ApplicationError> {
        config.validate()?;
        validate_admin_actor(&actor)?;
        validate_admin_secret(&secret)?;
        let secret_hash: [u8; 32] = Sha256::digest(secret).into();
        let mut token_digest = Hmac::<Sha256>::new_from_slice(&secret_hash)
            .map_err(|_| SessionError::InvalidConfiguration)?;
        token_digest.update(b"auction.admin-session.token.v2\0");
        token_digest.update(&(actor.len() as u64).to_be_bytes());
        token_digest.update(actor.as_bytes());
        store.revoke_all_admin(now).await?;
        Ok(Self { store, expected_actor: actor, secret_hash, token_digest, config })
    }

    /// 使用标准 HMAC 绑定随机 token 摘要；数据库仍只接收固定 32 字节，不接收凭据校验值。
    fn scoped_token_hash(&self, token_hash: [u8; 32]) -> [u8; 32] {
        let mut digest = self.token_digest.clone();
        digest.update(&token_hash);
        digest.finalize().into_bytes().into()
    }

    async fn authenticate_session(
        &self,
        raw_token: &str,
        raw_csrf_token: &str,
        now: Timestamp,
        renew: bool,
    ) -> Result<AdminSessionIdentity, ApplicationError> {
        let token_hash = self.scoped_token_hash(hash_token(raw_token)?);
        let csrf_hash = hash_token(raw_csrf_token)?;
        let identity = if renew {
            let next_expires_at = next_session_expiration(now, self.config.idle_ttl)?;
            self.store.touch_admin(token_hash, csrf_hash, now, next_expires_at).await?
        } else {
            self.store.inspect_admin(token_hash, csrf_hash, now).await?
        };
        if identity.actor != self.expected_actor {
            return Err(SessionError::Unauthenticated.into());
        }
        Ok(identity)
    }
}

impl AdminSessionPort for AdminSessionService {
    fn login(
        &self,
        actor: String,
        secret: Vec<u8>,
        now: Timestamp,
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

            let (token, token_hash) = generate_token();
            let token_hash = self.scoped_token_hash(token_hash);
            let (csrf_token, csrf_hash) = generate_token();
            let (expires_at, absolute_expires_at) =
                session_expiration_window(now, self.config.idle_ttl, self.config.absolute_ttl)?;
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
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(
            async move { self.authenticate_session(&raw_token, &raw_csrf_token, now, false).await },
        )
    }

    fn touch(
        &self,
        raw_token: String,
        raw_csrf_token: String,
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(
            async move { self.authenticate_session(&raw_token, &raw_csrf_token, now, true).await },
        )
    }

    fn logout(&self, raw_token: String, now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let token_hash = self.scoped_token_hash(hash_token(&raw_token)?);
            self.store.revoke_admin(token_hash, now).await
        })
    }
}

/// 统一计算会话过期时间；时间溢出必须失败关闭，不能写入不可持久化的时间。
fn session_expiration_window(
    now: Timestamp,
    idle_ttl: Duration,
    absolute_ttl: Duration,
) -> Result<(Timestamp, Timestamp), ApplicationError> {
    Ok((next_session_expiration(now, idle_ttl)?, next_session_expiration(now, absolute_ttl)?))
}

fn next_session_expiration(now: Timestamp, ttl: Duration) -> Result<Timestamp, ApplicationError> {
    now.checked_add_signed(ttl).ok_or_else(|| SessionError::InvalidConfiguration.into())
}

fn validate_login_key(key: &[u8]) -> Result<(), ApplicationError> {
    let value = std::str::from_utf8(key).map_err(|_| SessionError::InvalidLoginKey)?;
    if value.is_empty() || value.len() > 1024 || value != value.trim() {
        return Err(SessionError::InvalidLoginKey.into());
    }
    Ok(())
}

/// 校验配置与正式登录共同使用的 Admin 账号边界；错误不包含账号内容。
pub fn validate_admin_actor(actor: &str) -> Result<(), ApplicationError> {
    // Basic Auth 以首个冒号分隔账号与口令；带冒号的账号无法由 HTTP 登录入口表达。
    if !auction_domain::valid_text(actor, 256) || actor.contains(':') {
        return Err(SessionError::InvalidCredentials.into());
    }
    Ok(())
}

/// 校验配置与正式会话共同使用的 Admin 口令边界；错误不包含口令内容。
pub fn validate_admin_secret(secret: &[u8]) -> Result<(), ApplicationError> {
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
        || identity.user_state_version <= 0
        || !auction_domain::valid_text(&identity.username, 256)
    {
        return Err(SessionError::InvalidIdentity.into());
    }
    if !identity.active {
        let Some(reason) = identity.public_reason.as_deref() else {
            return Err(SessionError::InvalidCredentials.into());
        };
        if !auction_domain::valid_text(reason, 1024) {
            return Err(SessionError::InvalidCredentials.into());
        }
    }
    Ok(())
}

fn generate_token() -> (String, [u8; 32]) {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    let token = URL_SAFE_NO_PAD.encode(bytes);
    (token, Sha256::digest(bytes).into())
}

fn hash_token(raw_token: &str) -> Result<[u8; 32], ApplicationError> {
    // 固定 32 字节 token 的无填充编码恰为 43 字节；先拒绝其他长度，再解码到栈。
    if raw_token.len() != 43 {
        return Err(SessionError::Unauthenticated.into());
    }
    let mut decoded = [0_u8; 32];
    let decoded_len = URL_SAFE_NO_PAD
        .decode_slice(raw_token, &mut decoded)
        .map_err(|_| SessionError::Unauthenticated)?;
    if decoded_len != decoded.len() {
        return Err(SessionError::Unauthenticated.into());
    }
    Ok(Sha256::digest(decoded).into())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use auction_time::{Duration, Timestamp};

    use super::{AdminSessionService, generate_token};
    use crate::errors::{ApplicationError, SessionError};
    use crate::ports::core::ApplicationFuture;
    use crate::ports::public::{
        AdminSessionConfig, AdminSessionIdentity, AdminSessionPort, AdminSessionRecord,
        AdminSessionStore,
    };

    #[derive(Default)]
    struct MemorySession(Mutex<Option<AdminSessionRecord>>);

    impl MemorySession {
        fn authenticate(
            &self,
            token_hash: [u8; 32],
            csrf_hash: [u8; 32],
            now: Timestamp,
            next_expires_at: Option<Timestamp>,
        ) -> Result<AdminSessionIdentity, ApplicationError> {
            let mut row = self.0.lock().unwrap();
            let row = row.as_mut().ok_or(SessionError::Unauthenticated)?;
            if row.token_hash != token_hash
                || row.csrf_hash != csrf_hash
                || now >= row.expires_at.min(row.absolute_expires_at)
            {
                return Err(SessionError::Unauthenticated.into());
            }
            if let Some(next_expires_at) = next_expires_at {
                row.expires_at = row.expires_at.max(next_expires_at).min(row.absolute_expires_at);
            }
            Ok(identity(row))
        }
    }

    impl AdminSessionStore for MemorySession {
        fn revoke_all_admin(&self, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async move {
                self.0.lock().unwrap().take();
                Ok(())
            })
        }

        fn create_admin(
            &self,
            record: AdminSessionRecord,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move {
                let identity = identity(&record);
                self.0.lock().unwrap().replace(record);
                Ok(identity)
            })
        }

        fn inspect_admin(
            &self,
            token_hash: [u8; 32],
            csrf_hash: [u8; 32],
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move { self.authenticate(token_hash, csrf_hash, now, None) })
        }

        fn touch_admin(
            &self,
            token_hash: [u8; 32],
            csrf_hash: [u8; 32],
            now: Timestamp,
            next_expires_at: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(
                async move { self.authenticate(token_hash, csrf_hash, now, Some(next_expires_at)) },
            )
        }

        fn revoke_admin(&self, token_hash: [u8; 32], _now: Timestamp) -> ApplicationFuture<'_, ()> {
            Box::pin(async move {
                let mut row = self.0.lock().unwrap();
                if row.as_ref().is_some_and(|session| session.token_hash == token_hash) {
                    row.take();
                }
                Ok(())
            })
        }
    }

    fn identity(row: &AdminSessionRecord) -> AdminSessionIdentity {
        AdminSessionIdentity {
            actor: row.actor.clone(),
            expires_at: row.expires_at,
            absolute_expires_at: row.absolute_expires_at,
        }
    }

    fn unauthenticated(result: Result<AdminSessionIdentity, ApplicationError>) -> bool {
        matches!(result, Err(ApplicationError::Session(SessionError::Unauthenticated)))
    }

    #[test]
    fn admin_session_checks_and_renewals_share_validity_but_only_touch_changes_deadline() {
        let mut test = std::pin::pin!(check_admin_session());
        assert_eq!(test.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
    }

    async fn check_admin_session() {
        let now = Timestamp::now();
        let at = |seconds| now.checked_add_signed(Duration::seconds(seconds)).unwrap();
        let actor = "operator".to_owned();
        let secret = vec![b'a'; 32];
        let store = Arc::new(MemorySession::default());
        let service = AdminSessionService::new(
            store,
            actor.clone(),
            secret.clone(),
            AdminSessionConfig {
                idle_ttl: Duration::seconds(60),
                absolute_ttl: Duration::seconds(90),
            },
            now,
        )
        .await
        .unwrap();
        let login = service.login(actor.clone(), secret.clone(), now).await.unwrap();
        let read = service.current(login.token.clone(), login.csrf_token.clone(), at(45));
        assert_eq!(read.await.unwrap().expires_at, at(60));
        let touch = service.touch(login.token.clone(), login.csrf_token.clone(), at(45));
        assert_eq!(touch.await.unwrap().expires_at, at(90));
        let read = service.current(login.token.clone(), login.csrf_token.clone(), at(60));
        assert_eq!(read.await.unwrap().expires_at, at(90));

        let wrong_csrf = generate_token().0;
        assert!(unauthenticated(
            service.current(login.token.clone(), wrong_csrf.clone(), at(50)).await
        ));
        assert!(unauthenticated(service.touch(login.token.clone(), wrong_csrf, at(50)).await));
        assert!(unauthenticated(
            service.current(login.token.clone(), login.csrf_token.clone(), at(90)).await
        ));
        assert!(unauthenticated(service.touch(login.token, login.csrf_token, at(90)).await));

        let idle = service.login(actor, secret, now).await.unwrap();
        assert!(unauthenticated(
            service.current(idle.token.clone(), idle.csrf_token.clone(), at(60)).await
        ));
        assert!(unauthenticated(
            service.touch(idle.token.clone(), idle.csrf_token.clone(), at(60)).await
        ));
        service.logout(idle.token.clone(), at(1)).await.unwrap();
        assert!(unauthenticated(
            service.current(idle.token.clone(), idle.csrf_token.clone(), at(2)).await
        ));
        assert!(unauthenticated(service.touch(idle.token, idle.csrf_token, at(2)).await));
    }
}
