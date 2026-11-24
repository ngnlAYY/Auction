//! 单轮领取、开封、发送前复核、读取与观察提交。

use std::future::{Future, poll_fn};
use std::sync::Arc;
use std::task::Poll;

use auction_domain::CommandId;
use auction_domain::consignment::{ListingPublicationTtls, ListingReason, ListingVersion};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    ApplyListingLookupCommand, AutoCloseCandidate, AutoCloseEvidence, ConsignmentError,
    ListingBusinessRejection, ListingCommandOutcome, ListingCommandRejection, ListingCommandStore,
    ListingCredentialError, ListingEquipmentKeyOpener, ListingEquipmentLookup,
    ListingEquipmentLookupError, ListingLookupClaim, ListingLookupEffect, ListingOperation,
    LookupAbandonment, LookupClaimRequest, LookupDispatch, LookupLeaseUpdate, LookupObservation,
    LookupWorkStore, StoredListingDecision,
};
use crate::ports::core::{Clock, RequestHash};
use crate::services::consignment::encoding::{
    ListingCommandEncoding, auto_close_command, lookup_command_identity,
};

/// 一轮解析的结果及 Worker 调度依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingLookupRound {
    /// 本轮没领取到工作。可等待许可时间提示、新任务信号或有界轮询。
    Idle {
        /// 已知的全局许可下界；端口不提供待领队列的最早时刻。
        wake_at: Option<Timestamp>,
    },
    /// 外部访问暂停，等待恢复信号或有界轮询。
    Paused,
    /// 观察已有确定的提交结果，可重新领取其他工作。
    Applied(ListingLookupEffect),
    /// 系统已结束这项工作，Worker 应立即继续下一轮。
    WorkEnded,
    /// 原围栏已失效。只供计数和调度，不能重提本次观察或变更新许可。
    LeaseLost,
    /// Worker 正在排空；原许可已按请求状态让出或保留时间栅栏。
    Cancelled,
}

/// 单轮装备解析。数据库端口是租期、代次、冷却和资格的权威。
pub struct ResolveListingService {
    work: Arc<dyn LookupWorkStore>,
    commands: Arc<dyn ListingCommandStore>,
    opener: Arc<dyn ListingEquipmentKeyOpener>,
    lookup: Arc<dyn ListingEquipmentLookup>,
    paused: Arc<dyn Fn() -> bool + Send + Sync>,
    clock: Arc<dyn Clock>,
    publication_ttls: ListingPublicationTtls,
}

impl ResolveListingService {
    /// 生成解析事务内系统关闭的身份和摘要。独立补扫可用的证据沿用其规范编码。
    /// 只有当前账户失资格且没有新增拉黑事实时，才绑定此次解析命令与被关闭的版本。
    pub fn forced_close_identity(
        listing_id: Uuid,
        version: ListingVersion,
        reason: ListingReason,
        evidence: Option<AutoCloseEvidence>,
        lookup_command_id: CommandId,
    ) -> (CommandId, RequestHash) {
        if let Some(evidence) = evidence {
            let command = auto_close_command(AutoCloseCandidate { listing_id, evidence });
            return (command.command_id, command.request_hash);
        }
        // 不变量 1、2：账户当前状态没有补扫用的历史 UUID。用独立的证据标签绑定锁内版本和原观察。
        let mut input = ListingCommandEncoding::target_version(
            ListingOperation::AutoClose,
            listing_id,
            version,
        );
        input.text("lookup_forced_close");
        input.text(reason.as_str());
        input.bytes(lookup_command_id.as_uuid().as_bytes());
        let digest = input.request_hash();
        (CommandId::from_uuid(Uuid::new_v5(&listing_id, &digest.as_bytes())), digest)
    }

    /// 注入工作、写入及装备端口、现有外部暂停快照、本地出站时钟和发布期限。
    pub fn new(
        work: Arc<dyn LookupWorkStore>,
        commands: Arc<dyn ListingCommandStore>,
        opener: Arc<dyn ListingEquipmentKeyOpener>,
        lookup: Arc<dyn ListingEquipmentLookup>,
        paused: impl Fn() -> bool + Send + Sync + 'static,
        clock: Arc<dyn Clock>,
        publication_ttls: ListingPublicationTtls,
    ) -> Self {
        Self { work, commands, opener, lookup, paused: Arc::new(paused), clock, publication_ttls }
    }

    /// 跑一轮。请求应来自已同步的启用代次；排空时触发 `shutdown` 并等本轮返回。
    /// 已取得观察时等待提交。提交结果未知时只原样重提一次，不重新请求来源。
    pub async fn run_once(
        &self,
        request: LookupClaimRequest,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<ListingLookupRound, ConsignmentError> {
        if (self.paused)() {
            return Ok(ListingLookupRound::Paused);
        }
        let service_epoch = request.service_epoch();
        let requested_token = request.token();
        // 不变量 2、5、6：领取先持久计次，事务再次读取暂停，未领到时不消费凭证。
        let Some(claim) = self.work.claim(request).await? else {
            return self.idle().await;
        };
        if claim.service_epoch() != service_epoch || claim.fence().token() != requested_token {
            // 返回的领取不属于本请求时，无权开封或修改该围栏。
            return Ok(lease_lost(&claim));
        }
        let key = match claim.credential().and_then(|credential| self.opener.open(credential)) {
            Ok(key) => key,
            Err(ListingCredentialError) => {
                tracing::warn!(work_id = %claim.fence().work_id(), code = "credential_unavailable", "寄售解析凭证不可用");
                return self
                    .apply(
                        &claim,
                        LookupObservation::Failed(
                            ListingEquipmentLookupError::CredentialUnavailable,
                        ),
                    )
                    .await;
            }
        };

        if let LookupLeaseUpdate::Lost = self.work.renew(claim.fence()).await? {
            return Ok(lease_lost(&claim));
        }
        let paused = (self.paused)();
        let dispatch = self.work.confirm_dispatch(&claim, paused).await?;
        let deadline = match dispatch {
            LookupDispatch::Lost => {
                drop(key);
                self.abandon(&claim, LookupAbandonment::NotSent).await?;
                return Ok(lease_lost(&claim));
            }
            LookupDispatch::Allowed { deadline } if !paused && !(self.paused)() => {
                deadline.min(claim.deadline())
            }
            LookupDispatch::Paused | LookupDispatch::Allowed { .. } => {
                drop(key);
                self.abandon(&claim, LookupAbandonment::NotSent).await?;
                return Ok(ListingLookupRound::Paused);
            }
        };

        // 不变量 5、6、7：最后一次出站复核后不等待其他 I/O。本地时钟只可阻止发送，
        // 不能授予或延长已由数据库裁定的硬截止。
        if self.clock.now() >= deadline {
            drop(key);
            self.abandon(&claim, LookupAbandonment::NotSent).await?;
            return Ok(lease_lost(&claim));
        }
        let observation = until_shutdown(shutdown, async {
            match self.lookup.lookup(claim.identity(), &key, deadline).await {
                Ok(snapshot) => LookupObservation::Equipment(snapshot),
                Err(error) => LookupObservation::Failed(error),
            }
        })
        .await;
        drop(key);
        match observation {
            Ok(observation) => self.apply(&claim, observation).await,
            Err(status) => {
                self.abandon(&claim, status).await?;
                Ok(ListingLookupRound::Cancelled)
            }
        }
    }

    async fn idle(&self) -> Result<ListingLookupRound, ConsignmentError> {
        if (self.paused)() {
            return Ok(ListingLookupRound::Paused);
        }
        let status = self.work.permit_status().await?;
        let wake_at = [Some(status.next_allowed_at), status.cooldown_until, status.permit_until]
            .into_iter()
            .flatten()
            .filter(|until| *until > status.as_of)
            .max();
        Ok(ListingLookupRound::Idle { wake_at })
    }

    async fn abandon(
        &self,
        claim: &ListingLookupClaim,
        status: LookupAbandonment,
    ) -> Result<(), ConsignmentError> {
        // 不变量 5、6：使用原围栏让出，失效围栏不能解除后来的许可。
        if let LookupLeaseUpdate::Lost = self.work.abandon(claim.fence(), status).await? {
            lease_lost(claim);
        }
        Ok(())
    }

    async fn apply(
        &self,
        claim: &ListingLookupClaim,
        observation: LookupObservation,
    ) -> Result<ListingLookupRound, ConsignmentError> {
        let (command_id, request_hash) = lookup_command_identity(claim.fence(), &observation);
        // 不变量 1、2、3、5、6、7、8：原观察含 429 提示，资格、发布、重排、冷却、
        // 账本和许可的提交由同一事务决定。应用层不重发 GET，也不追加清退请求。
        let command = |observation| ApplyListingLookupCommand {
            request_hash,
            command_id,
            fence: claim.fence(),
            observation,
            publication_ttls: self.publication_ttls,
        };
        let original = match &observation {
            LookupObservation::Equipment(snapshot) => {
                LookupObservation::Equipment(snapshot.clone())
            }
            LookupObservation::Failed(error) => LookupObservation::Failed(*error),
        };
        let result = match self.commands.apply_lookup(command(observation)).await {
            Err(ConsignmentError::OutcomeUnknown) => {
                // 不变量 1、5：恢复未知提交只能重提同一命令和观察。若仍无法读到原决定，
                // 保留未知语义，交给持久租约回收；不能重新发网。
                match self.commands.apply_lookup(command(original)).await {
                    Err(ConsignmentError::Unavailable) => Err(ConsignmentError::OutcomeUnknown),
                    other => other,
                }
            }
            other => other,
        };
        if matches!(
            result,
            Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Recorded(
                ListingBusinessRejection::ListingNotActive
            ))) | Ok(ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(
                ListingBusinessRejection::ListingNotActive
            )))
        ) {
            return Ok(ListingLookupRound::WorkEnded);
        }
        match result.and_then(ListingCommandOutcome::into_result) {
            Ok(effect) => Ok(ListingLookupRound::Applied(effect)),
            Err(ConsignmentError::LeaseLost) => Ok(lease_lost(claim)),
            Err(error) => Err(error),
        }
    }
}

fn lease_lost(claim: &ListingLookupClaim) -> ListingLookupRound {
    tracing::debug!(work_id = %claim.fence().work_id(), code = "lease_lost", "寄售解析围栏已失效");
    ListingLookupRound::LeaseLost
}

async fn until_shutdown<T>(
    shutdown: impl Future<Output = ()>,
    request: impl Future<Output = T>,
) -> Result<T, LookupAbandonment> {
    let mut shutdown = std::pin::pin!(shutdown);
    let mut request = std::pin::pin!(request);
    let mut polled_request = false;
    poll_fn(|cx| {
        if shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(if polled_request {
                LookupAbandonment::MayBeInFlight
            } else {
                LookupAbandonment::NotSent
            }));
        }
        polled_request = true;
        request.as_mut().poll(cx).map(Ok)
    })
    .await
}
