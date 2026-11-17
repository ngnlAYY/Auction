//! Admin 手动 Cookie 检查：论坛 Cookie 在本进程用固定 HTTPS 只读端点检查；邮件 Cookie 只转交
//! Worker control，由 Worker 用与邮件循环共用请求门、在途锁和账号忙熔断的客户端执行。单并发、
//! 有界时限，不借用 Worker 写入会话、不建候选池、不记录远端正文。

use std::{sync::Arc, time::Duration};

use auction_application::admin::{
    AdminCredentialCheckObservation, AdminCredentialCheckRecord, AdminCredentialCheckStatus,
    AdminCredentialError, AdminCredentialKind, AdminCredentialOpener, AdminCredentialVerifier,
    credential_check_worlds,
};
use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::operations::{
    WorkerMailCredentialCheckPort, WorkerMailCredentialCheckRequest,
};
use tokio::sync::Semaphore;

use super::DecryptedCookie;
use crate::ehentai::{EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiErrorCategory};
use crate::safe_diagnostics;

/// 只接受数据库密文信封的 Admin 侧认证探测器；进程内不持有 HentaiVerse 客户端。
pub struct CredentialVerifier {
    opener: Arc<dyn AdminCredentialOpener>,
    forum: EhentaiClient,
    mail: Arc<dyn WorkerMailCredentialCheckPort>,
    slots: Semaphore,
}

impl CredentialVerifier {
    /// 固定生产论坛 origin，不允许管理请求自选 URL、代理或重定向策略；邮件 Cookie 只经注入的
    /// Worker control 转发端口检查，端口不可用时检查失败关闭为“暂不可用”。
    pub fn production(
        opener: Arc<dyn AdminCredentialOpener>,
        mail: Arc<dyn WorkerMailCredentialCheckPort>,
    ) -> Result<Self, ApplicationError> {
        let unavailable =
            || ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable);
        let mut forum = EhentaiClientConfig::production().map_err(safe_diagnostics::warn_map(
            "credential_check",
            "configure_forum",
            unavailable,
        ))?;
        forum.request_timeout = Duration::from_secs(4);
        forum.minimum_request_interval = Duration::from_secs(2);
        Ok(Self {
            opener,
            forum: EhentaiClient::with_config(forum).map_err(safe_diagnostics::warn_map(
                "credential_check",
                "construct_forum_client",
                unavailable,
            ))?,
            mail,
            slots: Semaphore::new(1),
        })
    }

    async fn check_forum(&self, secret: &[u8]) -> AdminCredentialCheckStatus {
        use AdminCredentialCheckStatus::{Inconclusive, Invalid, Valid};
        let Ok(cookie) = EhentaiCookie::from_bytes(secret) else { return Inconclusive };
        match self.forum.check_session(&cookie).await {
            Ok(()) => Valid,
            Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => Invalid,
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
            if credential.kind == AdminCredentialKind::MailCookie {
                // 邮件 Cookie 不在 Admin 解密，也不从 Admin 直连 HV：Worker 按 ID 重读信封、复核
                // 版本，并与自己的读写共用同账号在途锁与账号忙熔断。Worker 不可用时只报暂不可用。
                return self
                    .mail
                    .check_mail_credential(WorkerMailCredentialCheckRequest {
                        credential_id: credential.id,
                        expected_state_version: credential.state_version,
                    })
                    .await;
            }
            let secret = DecryptedCookie(self.opener.open(
                credential.kind,
                credential.id,
                &credential.world.scope(credential.kind),
                &record.envelope,
            )?);
            let mut checks = Vec::new();
            for world in credential_check_worlds(credential.kind, credential.world) {
                // 论坛登录态不区分世界，只有一个范围；最多五秒（含限速与正文），无重试。
                let status =
                    tokio::time::timeout(Duration::from_secs(5), self.check_forum(&secret.0))
                        .await
                        .unwrap_or(AdminCredentialCheckStatus::Inconclusive);
                checks.push(AdminCredentialCheckObservation { world: *world, status });
            }
            Ok(checks)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_application::admin::{
        AdminCredential, AdminCredentialCheckObservation, AdminCredentialCheckRecord,
        AdminCredentialCheckStatus, AdminCredentialEnvelope, AdminCredentialError,
        AdminCredentialKind, AdminCredentialOpener, AdminCredentialStatus, AdminCredentialVerifier,
        AdminCredentialWorld,
    };
    use auction_application::core::{ApplicationError, ApplicationFuture};
    use auction_application::operations::{
        UnavailableWorkerMailCredentialCheck, WorkerMailCredentialCheckPort,
        WorkerMailCredentialCheckRequest,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::CredentialVerifier;

    /// 邮件 Cookie 不得在 Admin 解密；被调用即说明转发路径泄漏到本地。
    struct ForbiddenOpener;

    impl AdminCredentialOpener for ForbiddenOpener {
        fn open(
            &self,
            _kind: AdminCredentialKind,
            _credential_id: Uuid,
            _scope: &str,
            _envelope: &AdminCredentialEnvelope,
        ) -> Result<Vec<u8>, ApplicationError> {
            panic!("邮件 Cookie 不得在 Admin 进程解密")
        }
    }

    /// 记录转发请求，按脚本回给 Admin。
    struct RecordingForward {
        result: Result<Vec<AdminCredentialCheckObservation>, AdminCredentialError>,
        requests: Mutex<Vec<WorkerMailCredentialCheckRequest>>,
    }

    impl WorkerMailCredentialCheckPort for RecordingForward {
        fn check_mail_credential(
            &self,
            request: WorkerMailCredentialCheckRequest,
        ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
            self.requests.lock().unwrap().push(request);
            let result = self.result.clone().map_err(ApplicationError::from);
            Box::pin(async move { result })
        }
    }

    fn mail_record(status: AdminCredentialStatus) -> AdminCredentialCheckRecord {
        AdminCredentialCheckRecord {
            credential: AdminCredential {
                id: Uuid::from_u128(42),
                kind: AdminCredentialKind::MailCookie,
                world: AdminCredentialWorld::Shared,
                label: "backup".to_owned(),
                player_name: None,
                status,
                state_version: 6,
                created_at: Timestamp::now(),
                rotated_at: None,
                updated_at: Timestamp::now(),
            },
            envelope: AdminCredentialEnvelope::new(
                vec![1; 32],
                vec![2; 12],
                "fixture-key".to_owned(),
                "fixture-fingerprint".to_owned(),
            )
            .expect("fixture envelope"),
        }
    }

    fn verifier(mail: Arc<dyn WorkerMailCredentialCheckPort>) -> CredentialVerifier {
        CredentialVerifier::production(Arc::new(ForbiddenOpener), mail).expect("verifier builds")
    }

    /// 指定的 disabled 凭据同样只按 ID 与版本转交 Worker，Admin 不解密、不直连。
    #[tokio::test]
    async fn mail_cookie_is_forwarded_by_identity_and_version_only() {
        let observations = vec![
            AdminCredentialCheckObservation {
                world: AdminCredentialWorld::Main,
                status: AdminCredentialCheckStatus::Valid,
            },
            AdminCredentialCheckObservation {
                world: AdminCredentialWorld::Isekai,
                status: AdminCredentialCheckStatus::Inconclusive,
            },
        ];
        let forward = Arc::new(RecordingForward {
            result: Ok(observations.clone()),
            requests: Mutex::new(Vec::new()),
        });
        let checks = verifier(forward.clone())
            .verify(mail_record(AdminCredentialStatus::Disabled))
            .await
            .expect("Worker 返回检查结果");
        assert_eq!(checks, observations);
        assert_eq!(
            *forward.requests.lock().unwrap(),
            vec![WorkerMailCredentialCheckRequest {
                credential_id: Uuid::from_u128(42),
                expected_state_version: 6,
            }]
        );
    }

    /// Worker 不可用时失败关闭为“暂不可用”，不回退到 Admin 直连；Worker 的业务拒绝原样返回。
    #[tokio::test]
    async fn worker_unavailability_and_rejections_never_fall_back_to_a_direct_check() {
        let error = verifier(Arc::new(UnavailableWorkerMailCredentialCheck))
            .verify(mail_record(AdminCredentialStatus::Active))
            .await
            .expect_err("Worker 不可用");
        assert!(
            matches!(
                error,
                ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable)
            ),
            "{error:?}"
        );

        let forward = Arc::new(RecordingForward {
            result: Err(AdminCredentialError::VersionConflict { current_version: 7 }),
            requests: Mutex::new(Vec::new()),
        });
        let error = verifier(forward)
            .verify(mail_record(AdminCredentialStatus::Active))
            .await
            .expect_err("版本已变化");
        assert!(
            matches!(
                error,
                ApplicationError::AdminCredential(AdminCredentialError::VersionConflict {
                    current_version: 7
                })
            ),
            "{error:?}"
        );
    }
}
