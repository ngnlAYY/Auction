//! E-Hentai 单 Cookie 网关与 Forum 端口适配。

use std::fmt;
use std::sync::Arc;

use auction_application::admin::AdminForumPreviewResult;
use auction_application::core::ApplicationError;
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::forum::{
    ForumCredentialAvailability, ForumPostWriter, ForumPublishedPost, ForumPublishedThread,
    ForumPublisher, ForumReadPost, ForumRenderedPost, ForumRenderedThread, ForumTopicMetadata,
    ForumTopicReader,
};
use auction_domain::SettlementWorld;

use super::client::EhentaiClient;
use super::credentials::{EhentaiCredentialSession, EhentaiCredentialSet};
use super::error::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};
use crate::clients::metrics::{
    ForumPmOperationGuard, ForumWriteUnknownGuard, forum_pm_category, forum_world,
};
use crate::safe_diagnostics;

/// 使用唯一论坛 Cookie；明确失效会回写，不自动换号或重放外部写入。
#[derive(Clone)]
pub struct EhentaiGateway {
    client: Arc<EhentaiClient>,
    credentials: EhentaiCredentialSet,
}

impl EhentaiGateway {
    /// 建立不读取凭据、不发起网络请求的网关。
    pub fn new(
        client: Arc<EhentaiClient>,
        credentials: EhentaiCredentialSet,
    ) -> Result<Self, EhentaiError> {
        // 数据库凭据允许暂时为空；真实操作会返回 credential_unavailable，Worker 仍保持 degraded。
        let client = Arc::new(client.as_ref().clone().with_ownership(credentials.ownership()));
        Ok(Self { client, credentials })
    }

    /// 创建主题；读侧会话失效返回凭据不可用，写后失效保留原写入未知标记。
    pub async fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> Result<auction_application::forum::ForumTopicPublication, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let metrics_world = forum_world(world);
        let write_unknown = ForumWriteUnknownGuard::start(
            self.client.external_metrics(),
            metrics_world,
            auction_application::external_metrics::ForumOperation::CreateTopic,
            progress.clone(),
        );
        let client = self.client.as_ref().clone().with_write_progress(progress);
        let result = client.create_topic(world, session.cookie(), rendered).await;
        let result = settle_session(&mut session, EhentaiOperation::CreateTopic, result).await;
        write_unknown.finish(&result);
        result
    }

    /// 只读按有界精确标题集合查找唯一主题；会话失效后停止，不切换 Cookie。
    pub async fn find_topic_by_titles(
        &self,
        world: SettlementWorld,
        titles: &[String],
    ) -> Result<Option<auction_application::forum::ForumPublishedThread>, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let result = self.client.find_topic_by_titles(world, session.cookie(), titles).await;
        settle_session(&mut session, EhentaiOperation::ReconcileTopic, result).await
    }

    /// 读取主题；明确会话失效时回写并返回，不重试当前逻辑操作。
    pub async fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
    ) -> Result<AdminForumPreviewResult, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let result = self.client.fetch_topic(world, thread_id, session.cookie()).await;
        settle_session(&mut session, EhentaiOperation::FetchTopic, result).await
    }

    /// 按游标读取主题；游标页不再包含该帖子时客户端自行回退完整读取。
    pub async fn fetch_topic_incremental(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        cursor: Option<&auction_application::forum::ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> Result<auction_application::forum::ForumTopicScan, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let result = self
            .client
            .fetch_topic_incremental(
                world,
                thread_id,
                session.cookie(),
                cursor,
                fixed_through_post_number,
            )
            .await;
        settle_session(&mut session, EhentaiOperation::FetchTopic, result).await
    }

    /// 建立目录续页或系统日志；正文只移交一次，POST 开始后保持写入未知边界。
    pub async fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        body: ForumRenderedPost,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let metrics_world = forum_world(world);
        let write_unknown = ForumWriteUnknownGuard::start(
            self.client.external_metrics(),
            metrics_world,
            auction_application::external_metrics::ForumOperation::ReplyTopic,
            progress.clone(),
        );
        let client = self.client.as_ref().clone().with_write_progress(progress);
        let result = client.reply_topic(world, thread_id, body, session.cookie()).await;
        let result = settle_session(&mut session, EhentaiOperation::ReplyTopic, result).await;
        write_unknown.finish(&result);
        result
    }

    /// 编辑固定目录页或系统日志；POST 开始后绝不切换 Cookie 重发。
    pub async fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
        metadata: Option<&ForumTopicMetadata>,
        body: ForumRenderedPost,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> Result<ForumPublishedPost, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let metrics_world = forum_world(world);
        let write_unknown = ForumWriteUnknownGuard::start(
            self.client.external_metrics(),
            metrics_world,
            auction_application::external_metrics::ForumOperation::EditPost,
            progress.clone(),
        );
        let client = self.client.as_ref().clone().with_write_progress(progress);
        let result =
            client.edit_post(world, thread_id, post_id, metadata, body, session.cookie()).await;
        let result = settle_session(&mut session, EhentaiOperation::EditPost, result).await;
        write_unknown.finish(&result);
        result
    }

    /// 只读回读固定楼层原始 BBCode 与首帖元数据；明确失效后返回凭据不可用。
    pub async fn read_post(
        &self,
        world: SettlementWorld,
        thread_id: &str,
        post_id: &str,
    ) -> Result<ForumReadPost, EhentaiError> {
        let mut session = self.credentials.begin(Some(world))?;
        let result = self.client.read_post(world, thread_id, post_id, session.cookie()).await;
        settle_session(&mut session, EhentaiOperation::ReadPost, result).await
    }
}

/// 收口一次逻辑操作的会话失效：明确失效都撤销并回写当前 Cookie，不换号重试。
/// 尚未写出的失效映射为凭据不可用；写入开始后的失效原样保留，`write_started` 栅栏不变，
/// 上层只能先对账，不能当作未发送重发（不变量 4）。回写失败已在会话内部记录诊断，
/// 进程内撤销仍然生效，因此这里不改变返回结果。
async fn settle_session<T>(
    session: &mut EhentaiCredentialSession,
    operation: EhentaiOperation,
    result: Result<T, EhentaiError>,
) -> Result<T, EhentaiError> {
    match result {
        Err(error) if error.category() == EhentaiErrorCategory::SessionExpired => {
            let _ = session.invalidate_current().await;
            if error.write_started() {
                Err(error)
            } else {
                Err(EhentaiError::credential(operation))
            }
        }
        result => result,
    }
}

impl auction_application::forum::ForumTargetProfileReader for EhentaiGateway {
    fn read_target_profile(
        &self,
        target_uid: String,
        deadline: std::time::Instant,
    ) -> auction_application::settlement::InternalEvidenceFuture<
        '_,
        auction_application::forum::ForumTargetProfileEvidence,
    > {
        Box::pin(async move {
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                let mut session = self.credentials.begin(None).map_err(safe_diagnostics::warn_map(
                    "forum_profile",
                    "acquire_session",
                    || auction_application::settlement::InternalEvidenceError::Unavailable,
                ))?;
                let result = self.client.read_target_profile(&target_uid, session.cookie()).await;
                // 资料读取已明确证明当前 Cookie 失效（登录页或 owner 撤销）时停止复用：
                // 不撤销会在后续读取中反复送出同一失效凭据。
                if matches!(
                    &result,
                    Err(auction_application::settlement::InternalEvidenceError::Revoked)
                ) {
                    let _ = session.invalidate_current().await;
                }
                result
            })
            .await
            .map_err(|diagnostic_cause| {
                self.client.external_metrics().observe(
                    auction_application::external_metrics::ExternalMetricsEvent::ForumBudgetExhausted {
                        world: auction_application::external_metrics::ForumWorld::Shared,
                        phase: auction_application::external_metrics::ForumBudgetPhase::Profile,
                    },
                );
                safe_diagnostics::warn(
                    "forum_profile",
                    "read_profile_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                auction_application::settlement::InternalEvidenceError::Expired
            })?
        })
    }
}

impl auction_application::forum::ForumPmSender for EhentaiGateway {
    fn prepare<'a>(
        &'a self,
        world: SettlementWorld,
        message: &'a auction_application::forum::ForumPmMessage,
    ) -> auction_application::forum::ForumPmFuture<
        'a,
        Box<dyn auction_application::forum::PreparedForumPm>,
    > {
        Box::pin(async move {
            let metrics_world = forum_world(world);
            let operation = ForumPmOperationGuard::start(
                self.client.external_metrics(),
                metrics_world,
                auction_application::external_metrics::ForumPmPhase::Prepare,
                false,
            );
            let session = match self.credentials.begin(Some(world)) {
                Ok(session) => session,
                Err(diagnostic_cause) => {
                    safe_diagnostics::warn(
                        "forum_pm",
                        "acquire_send_session",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    operation.finish(
                        auction_application::external_metrics::ForumPmOutcome::Error,
                        auction_application::external_metrics::ForumPmCategory::Unavailable,
                    );
                    return Err(auction_application::forum::ForumPmError::Unavailable);
                }
            };
            let result = self.client.prepare_pm(message, session, metrics_world).await;
            match &result {
                Ok(_) => operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::Prepared,
                    auction_application::external_metrics::ForumPmCategory::None,
                ),
                Err(error) => operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::Error,
                    forum_pm_category(*error),
                ),
            }
            result
        })
    }

    fn reconcile<'a>(
        &'a self,
        world: SettlementWorld,
        message: &'a auction_application::forum::ForumPmMessage,
        sender_forum_account_id: &'a str,
    ) -> auction_application::forum::ForumPmFuture<
        'a,
        Option<auction_application::forum::ForumPmReceipt>,
    > {
        Box::pin(async move {
            let metrics_world = forum_world(world);
            let operation = ForumPmOperationGuard::start(
                self.client.external_metrics(),
                metrics_world,
                auction_application::external_metrics::ForumPmPhase::Reconcile,
                false,
            );
            let session = match self.credentials.begin(Some(world)) {
                Ok(session) => session,
                Err(diagnostic_cause) => {
                    safe_diagnostics::warn(
                        "forum_pm",
                        "acquire_reconcile_session",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    operation.finish(
                        auction_application::external_metrics::ForumPmOutcome::Error,
                        auction_application::external_metrics::ForumPmCategory::Unavailable,
                    );
                    return Err(auction_application::forum::ForumPmError::Unavailable);
                }
            };
            let result = self
                .client
                .reconcile_pm(message, sender_forum_account_id, session, metrics_world)
                .await;
            match &result {
                Ok(Some(_)) => operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::Proved,
                    auction_application::external_metrics::ForumPmCategory::None,
                ),
                Ok(None) => operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::NotObserved,
                    auction_application::external_metrics::ForumPmCategory::None,
                ),
                Err(error) => operation.finish(
                    auction_application::external_metrics::ForumPmOutcome::Error,
                    forum_pm_category(*error),
                ),
            }
            result
        })
    }
}

impl ForumCredentialAvailability for EhentaiGateway {
    fn has_available_forum_credential(&self, world: SettlementWorld) -> bool {
        self.credentials.available(world)
    }
}

impl fmt::Debug for EhentaiGateway {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EhentaiGateway")
            .field("client", &self.client)
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl ForumPublisher for EhentaiGateway {
    fn find_topic_by_titles(
        &self,
        world: SettlementWorld,
        titles: Vec<String>,
    ) -> ApplicationFuture<'_, Option<ForumPublishedThread>> {
        Box::pin(async move {
            EhentaiGateway::find_topic_by_titles(self, world, &titles)
                .await
                .map_err(observed_application_error)
        })
    }

    fn create_topic(
        &self,
        world: SettlementWorld,
        rendered: ForumRenderedThread,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> ApplicationFuture<'_, auction_application::forum::ForumTopicPublication> {
        Box::pin(async move {
            EhentaiGateway::create_topic(self, world, rendered, progress)
                .await
                .map_err(observed_application_error)
        })
    }
}

impl ForumPostWriter for EhentaiGateway {
    fn reply_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
        body: ForumRenderedPost,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        Box::pin(async move {
            EhentaiGateway::reply_topic(self, world, &thread_id, body, progress)
                .await
                .map_err(observed_application_error)
        })
    }

    fn edit_post(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
        metadata: Option<ForumTopicMetadata>,
        body: ForumRenderedPost,
        progress: auction_application::forum::ForumWriteProgress,
    ) -> ApplicationFuture<'_, ForumPublishedPost> {
        Box::pin(async move {
            EhentaiGateway::edit_post(
                self,
                world,
                &thread_id,
                &post_id,
                metadata.as_ref(),
                body,
                progress,
            )
            .await
            .map_err(observed_application_error)
        })
    }

    fn read_post(
        &self,
        world: SettlementWorld,
        thread_id: String,
        post_id: String,
    ) -> ApplicationFuture<'_, ForumReadPost> {
        Box::pin(async move {
            EhentaiGateway::read_post(self, world, &thread_id, &post_id)
                .await
                .map_err(observed_application_error)
        })
    }
}

impl ForumTopicReader for EhentaiGateway {
    fn fetch_topic(
        &self,
        world: SettlementWorld,
        thread_id: String,
    ) -> ApplicationFuture<'_, AdminForumPreviewResult> {
        Box::pin(async move {
            EhentaiGateway::fetch_topic(self, world, &thread_id)
                .await
                .map_err(observed_application_error)
        })
    }

    fn fetch_topic_since(
        &self,
        world: SettlementWorld,
        thread_id: String,
        cursor: Option<auction_application::forum::ForumTopicCursor>,
        fixed_through_post_number: u32,
    ) -> ApplicationFuture<'_, auction_application::forum::ForumTopicScan> {
        Box::pin(async move {
            EhentaiGateway::fetch_topic_incremental(
                self,
                world,
                &thread_id,
                cursor.as_ref(),
                fixed_through_post_number,
            )
            .await
            .map_err(observed_application_error)
        })
    }
}

fn observed_application_error(error: EhentaiError) -> ApplicationError {
    let (kind, phase) = match error.operation() {
        super::EhentaiOperation::CheckSession => ("credential_check", "check_forum_session"),
        super::EhentaiOperation::CreateTopic => ("forum_topic", "create_topic"),
        super::EhentaiOperation::ReconcileTopic => ("forum_topic", "reconcile_topic"),
        super::EhentaiOperation::FetchTopic => ("forum_topic", "fetch_topic"),
        super::EhentaiOperation::ReplyTopic => ("forum_topic", "reply_topic"),
        super::EhentaiOperation::EditPost => ("forum_topic", "edit_post"),
        super::EhentaiOperation::ReadPost => ("forum_topic", "read_post"),
    };
    safe_diagnostics::warn(
        kind,
        phase,
        safe_diagnostics::project(&error),
        DiagnosticContext::default(),
    );
    error.into_application_error()
}

#[cfg(test)]
mod tests {
    use auction_domain::SettlementWorld;

    use super::settle_session;
    use crate::clients::ehentai::credentials::{EhentaiCookie, EhentaiCredentialSet};
    use crate::clients::ehentai::error::{EhentaiError, EhentaiErrorCategory, EhentaiOperation};

    fn credentials() -> EhentaiCredentialSet {
        let cookie = EhentaiCookie::from_bytes(b"ipb_member_id=fixture; ipb_pass_hash=fixture")
            .expect("fixture cookie");
        EhentaiCredentialSet::from_cookies(vec![cookie]).expect("credential set")
    }

    #[tokio::test]
    async fn read_side_expiry_revokes_cookie_and_reports_credential_unavailable() {
        let set = credentials();
        let mut session = set.begin(Some(SettlementWorld::Main)).expect("session");
        let result: Result<(), EhentaiError> =
            Err(EhentaiError::session_expired(EhentaiOperation::FetchTopic));

        let error = settle_session(&mut session, EhentaiOperation::FetchTopic, result)
            .await
            .expect_err("失效必须失败");
        assert_eq!(error.category(), EhentaiErrorCategory::CredentialUnavailable);
        assert!(!error.write_started());
        assert!(!set.available(SettlementWorld::Main), "失效 Cookie 必须撤销");
    }

    #[tokio::test]
    async fn post_write_expiry_revokes_cookie_and_keeps_write_started() {
        let set = credentials();
        let mut session = set.begin(Some(SettlementWorld::Main)).expect("session");
        let expired = EhentaiError::new(
            EhentaiOperation::ReplyTopic,
            EhentaiErrorCategory::SessionExpired,
            false,
            true,
        );

        let error =
            settle_session(&mut session, EhentaiOperation::ReplyTopic, Err::<(), _>(expired))
                .await
                .expect_err("失效必须失败");
        // 写后失效保留原错误与写入栅栏，上层只能对账，不能当作未发送重发。
        assert_eq!(error, expired);
        assert!(error.write_started());
        assert!(!set.available(SettlementWorld::Main), "失效 Cookie 必须撤销");
    }

    #[tokio::test]
    async fn other_outcomes_pass_through_without_revoking() {
        let set = credentials();
        let mut session = set.begin(Some(SettlementWorld::Main)).expect("session");
        let temporary = EhentaiError::remote_temporary(EhentaiOperation::FetchTopic, false);

        let error =
            settle_session(&mut session, EhentaiOperation::FetchTopic, Err::<(), _>(temporary))
                .await
                .expect_err("原错误透传");
        assert_eq!(error, temporary);
        let value = settle_session(&mut session, EhentaiOperation::FetchTopic, Ok(7)).await;
        assert_eq!(value, Ok(7));
        assert!(set.available(SettlementWorld::Main), "非会话失效不撤销 Cookie");
    }
}
