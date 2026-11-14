//! Admin 手动 Cookie 检查：固定 HTTPS 只读端点、单并发与有界时限；不借用 Worker
//! 写入会话、不建候选池、不记录远端正文。

use std::{sync::Arc, time::Duration};

use auction_application::admin::{
    AdminCredentialCheckObservation, AdminCredentialCheckRecord, AdminCredentialCheckStatus,
    AdminCredentialError, AdminCredentialKind, AdminCredentialOpener, AdminCredentialVerifier,
    AdminCredentialWorld, credential_check_worlds,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticContext;
use chrono::Utc;
use tokio::sync::Semaphore;

use crate::safe_diagnostics;
use crate::{
    ehentai::{EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiErrorCategory},
    mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailErrorCategory,
        MoogleMailParser, MoogleMailWorld,
    },
};

/// 只接受数据库密文信封的临时认证探测器。
pub struct CredentialVerifier {
    opener: Arc<dyn AdminCredentialOpener>,
    forum: EhentaiClient,
    game: MoogleMailClient,
    slots: Semaphore,
}

impl CredentialVerifier {
    /// 固定生产 origin；不允许管理请求自选 URL、代理或重定向策略。
    pub fn production(opener: Arc<dyn AdminCredentialOpener>) -> Result<Self, ApplicationError> {
        let unavailable =
            || ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable);
        let mut forum = EhentaiClientConfig::production().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_check",
                "configure_forum",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        forum.request_timeout = Duration::from_secs(4);
        forum.minimum_request_interval = Duration::from_secs(2);
        let mut game = MoogleMailClientConfig::production().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_check",
                "configure_game",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        game.request_timeout = Duration::from_secs(4);
        game.minimum_request_interval = Duration::from_secs(2);
        game.maximum_response_bytes = 2 * 1024 * 1024;
        Ok(Self {
            opener,
            forum: EhentaiClient::with_config(forum).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_check",
                    "construct_forum_client",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?,
            game: MoogleMailClient::with_config(game).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_check",
                    "construct_game_client",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?,
            slots: Semaphore::new(1),
        })
    }

    async fn check_one(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
        secret: &[u8],
    ) -> AdminCredentialCheckStatus {
        use AdminCredentialCheckStatus::{Inconclusive, Invalid, Valid};
        match kind {
            AdminCredentialKind::ForumCookie => {
                let Ok(cookie) = EhentaiCookie::from_bytes(secret) else { return Inconclusive };
                match self.forum.check_session(&cookie).await {
                    Ok(()) => Valid,
                    Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
                        Invalid
                    }
                    Err(diagnostic_cause) => {
                        safe_diagnostics::warn(
                            "credential_check",
                            "verify_forum_session",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        Inconclusive
                    }
                }
            }
            AdminCredentialKind::MailCookie => {
                let world = match world {
                    AdminCredentialWorld::Main => MoogleMailWorld::Main,
                    AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
                    AdminCredentialWorld::Shared => return Inconclusive,
                };
                let Ok(secret) = std::str::from_utf8(secret) else {
                    tracing::debug!(
                        world = world.as_str(),
                        reason = "not_utf8",
                        "mail credential check inconclusive"
                    );
                    return Inconclusive;
                };
                let Ok(cookie) = MoogleMailCookie::new(secret) else {
                    tracing::debug!(
                        world = world.as_str(),
                        reason = "invalid_cookie",
                        "mail credential check inconclusive"
                    );
                    return Inconclusive;
                };
                let Ok(target) = self.game.balance(world) else {
                    tracing::debug!(
                        world = world.as_str(),
                        reason = "invalid_target",
                        "mail credential check inconclusive"
                    );
                    return Inconclusive;
                };
                // Item Shop 只读余额页不触发邮件打开、取附件、compose、send 或战斗操作。
                // Valid 只表示当次 Cookie 可用；余额不是自身名称证据，不能生成 VerifiedMailPlayerIdentity。
                match self.game.get(&target, &cookie).await {
                    Ok(body) => match MoogleMailParser.parse_balance(&body, world, Utc::now()) {
                        Ok(_) => Valid,
                        Err(diagnostic_cause) => {
                            safe_diagnostics::warn(
                                "credential_check",
                                "parse_game_identity",
                                safe_diagnostics::project(&diagnostic_cause),
                                DiagnosticContext::default(),
                            );
                            Inconclusive
                        }
                    },
                    Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                        Invalid
                    }
                    Err(diagnostic_cause) => {
                        safe_diagnostics::warn(
                            "credential_check",
                            "verify_game_session",
                            safe_diagnostics::project(&diagnostic_cause),
                            DiagnosticContext::default(),
                        );
                        Inconclusive
                    }
                }
            }
        }
    }
}

impl AdminCredentialVerifier for CredentialVerifier {
    fn verify(
        &self,
        record: AdminCredentialCheckRecord,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
        Box::pin(async move {
            // 立即拒绝重叠检查，避免排队请求在取消后仍消耗 Cookie 或远端配额。
            let _permit = self.slots.try_acquire().map_err(|_| AdminCredentialError::CheckBusy)?;
            let credential = &record.credential;
            let secret = DecryptedCookie(self.opener.open(
                credential.kind,
                credential.id,
                &credential.world.scope(credential.kind),
                &record.envelope,
            )?);
            let mut checks = Vec::new();
            for world in credential_check_worlds(credential.kind, credential.world) {
                // 每个范围最多五秒（含限速与正文）；shared 最多两个 GET，无重试。
                let status = tokio::time::timeout(
                    Duration::from_secs(5),
                    self.check_one(credential.kind, *world, &secret.0),
                )
                .await
                .unwrap_or(AdminCredentialCheckStatus::Inconclusive);
                checks.push(AdminCredentialCheckObservation { world: *world, status });
            }
            Ok(checks)
        })
    }
}

/// 取消或错误退出也清空解密缓冲；不实现 Debug/Serialize。
struct DecryptedCookie(Vec<u8>);

impl Drop for DecryptedCookie {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
