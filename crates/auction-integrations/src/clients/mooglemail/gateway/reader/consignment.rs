//! 在原认证会话内读取寄售来信：绑定 Bot UID、完整玩家名、世界与凭据代际，产出可隔离观察。
//!
//! 读失败只有闭集分类；临时限流、战斗、allowlist 与无法建立 UID 保留 pending，
//! 认证成功但清单不明则返回带未判标记的观察，绝不变成普通空信。

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::{MailPollClaim, MailPollReadJournal, MailPollReadSummary};
use auction_application::settlement::{InternalEvidenceError, MailAccountEvidenceBinding};
use auction_domain::consignment::{LegacyAuctionOwner, SenderIdentity};
use auction_time::Timestamp;
use uuid::Uuid;

use super::super::credentials::MoogleMailCredentialSession;
use super::inbox::{ConsignmentReadClassification, classify_consignment_read_error};
use crate::clients::mooglemail::MoogleMailOperation;
use crate::clients::mooglemail::MoogleMailTarget;
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::gateway::MoogleMailGateway;
use crate::clients::mooglemail::parser::{
    ConsignmentDetailObservation, ConsignmentListRow, FrozenMailManifest, MoogleMailActionState,
};
use crate::clients::mooglemail::protocol::valid_opaque_id;
use crate::clients::mooglemail::transport::{
    DispositionAction, DispositionRequest, NotSentReason, manifest_matches,
};
use crate::safe_diagnostics;

/// 寄售读取保持 pending 的闭集原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsignmentReadPending {
    /// 临时限流或证据预算暂时耗尽。
    RateLimited,
    /// 账号处于战斗页。
    AccountBusy,
    /// 完整发件人未通过既有 allowlist。
    AllowlistDenied,
    /// 尚不能建立认证 UID、完整玩家名或世界绑定。
    AuthenticatedUidUnavailable,
    /// 网络、超时、会话失效等暂时不可读。
    TransientReadUnavailable,
}

impl ConsignmentReadPending {
    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::AccountBusy => "account_busy",
            Self::AllowlistDenied => "allowlist_denied",
            Self::AuthenticatedUidUnavailable => "authenticated_uid_unavailable",
            Self::TransientReadUnavailable => "transient_read_unavailable",
        }
    }
}

/// 一次寄售读取的结果闭集。
#[derive(Debug)]
pub enum ConsignmentReadOutcome {
    /// 已认证并形成受控观察；未判标记在观察内部。
    Observed(Box<BoundConsignmentObservation>),
    /// 保留原 pending 定位，稍后重读。
    Pending(ConsignmentReadPending),
}

/// 绑定到认证会话的寄售原观察。
pub struct BoundConsignmentObservation {
    pub(super) observation: ConsignmentDetailObservation,
    pub(super) binding: MailAccountEvidenceBinding,
    pub(super) remote_observation_id: Uuid,
    pub(super) observed_at: Timestamp,
}

impl fmt::Debug for BoundConsignmentObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundConsignmentObservation")
            .field("observation", &self.observation)
            .field("binding", &self.binding)
            .field("remote_observation_id", &self.remote_observation_id)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

impl BoundConsignmentObservation {
    /// 原始观察。
    pub fn observation(&self) -> &ConsignmentDetailObservation {
        &self.observation
    }

    /// 认证会话绑定。
    pub fn binding(&self) -> &MailAccountEvidenceBinding {
        &self.binding
    }

    /// 远端观察身份；同 K、同清单重放得到同一值。
    pub fn remote_observation_id(&self) -> Uuid {
        self.remote_observation_id
    }

    /// 本次读取时刻。
    pub fn observed_at(&self) -> Timestamp {
        self.observed_at
    }

    /// 会话绑定摘要；进入 v2 命令与持久观察。
    pub fn session_binding_digest(&self) -> [u8; 32] {
        self.binding.digest
    }

    /// 完全可证明时的冻结清单；未判形状返回 `None`。
    pub fn frozen_manifest(&self) -> Option<FrozenMailManifest> {
        self.observation.frozen_manifest()
    }

    /// 结合既往认证身份判断同名换 UID、改名续办或无法证明。
    pub fn mailbox_identity(
        &self,
        previous: Option<&MailAccountEvidenceBinding>,
    ) -> SenderIdentity {
        if self.observation.recipient != self.binding.player_name {
            return SenderIdentity::Unverified;
        }
        match previous {
            Some(previous)
                if previous.player_name == self.binding.player_name
                    && previous.authenticated_uid != self.binding.authenticated_uid =>
            {
                SenderIdentity::SameNameDifferentUid
            }
            Some(previous)
                if previous.authenticated_uid == self.binding.authenticated_uid
                    && previous.player_name != self.binding.player_name =>
            {
                SenderIdentity::Verified
            }
            Some(previous)
                if previous.authenticated_uid != self.binding.authenticated_uid
                    && previous.player_name != self.binding.player_name =>
            {
                SenderIdentity::Unverified
            }
            _ => SenderIdentity::Verified,
        }
    }

    /// 转成 4A 领域观察；旧拍卖 owner 由应用层先运行拍卖路由后填入。
    pub fn to_envelope_observation(
        &self,
        observation_id: Uuid,
        previous: Option<&MailAccountEvidenceBinding>,
        legacy_auction_owner: Option<LegacyAuctionOwner>,
    ) -> auction_domain::consignment::EnvelopeObservation {
        self.observation.to_envelope_observation(
            observation_id,
            self.binding.digest,
            self.mailbox_identity(previous),
            self.observation.system_rts(),
            legacy_auction_owner,
        )
    }
}

impl MoogleMailGateway {
    /// 读取一封寄售来信的详情；列表行提供原始分钟与列表投影。
    ///
    /// 详情 GET 会改变远端已读状态，所以必须先在同一 `claim` 上登记待读窗口；
    /// 无法登记或已有更早的其他待读邮件时零 GET，保留 pending 等待重读。
    /// 不变量 4、5：本地未记录待读定位不得先改变远端状态。
    pub async fn read_consignment_envelope(
        &self,
        row: &ConsignmentListRow,
        deadline: Instant,
        journal: &dyn MailPollReadJournal,
        claim: &MailPollClaim,
    ) -> ConsignmentReadOutcome {
        if !valid_opaque_id(&row.external_mail_id)
            || deadline <= Instant::now()
            || !claim.validate()
        {
            return ConsignmentReadOutcome::Pending(
                ConsignmentReadPending::TransientReadUnavailable,
            );
        }
        let world = row.world;
        // 只按持久待读前缀的顺序读取；若更早的待读不属于当前行，不能跳过它先读详情。
        match journal.pending_reads(claim.clone(), world.as_str().to_owned(), 1).await {
            Ok(pending)
                if pending
                    .first()
                    .is_some_and(|first| first.external_mail_id != row.external_mail_id) =>
            {
                return ConsignmentReadOutcome::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                );
            }
            Ok(_) => {}
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "pending_consignment_reads",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                return ConsignmentReadOutcome::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                );
            }
        }
        let summary = MailPollReadSummary {
            external_mail_id: row.external_mail_id.clone(),
            world_id: world.as_str().to_owned(),
            sender: row.sender.clone(),
            subject: row.subject.clone(),
            received_at: row.received_at,
            list_world_marker: row.world_marker.map(|marker| marker.as_str().to_owned()),
            received_minute_raw: Some(row.received_minute_raw.clone()),
        };
        if let Err(diagnostic_cause) =
            journal.stage_reads(claim.clone(), world.as_str().to_owned(), vec![summary]).await
        {
            safe_diagnostics::warn(
                "mooglemail_read",
                "stage_consignment_read",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            return ConsignmentReadOutcome::Pending(
                ConsignmentReadPending::TransientReadUnavailable,
            );
        }
        let session =
            match self.credentials.begin_exclusive(world, MoogleMailOperation::Detail).await {
                Ok(session) => session,
                Err(error) => return pending_from_error(error),
            };
        let binding = match self.read_identity_in_session(&session, world, deadline).await {
            Ok(binding) => binding,
            Err(InternalEvidenceError::Unavailable | InternalEvidenceError::Expired)
            | Err(InternalEvidenceError::Revoked) => {
                return ConsignmentReadOutcome::Pending(
                    ConsignmentReadPending::TransientReadUnavailable,
                );
            }
            Err(
                InternalEvidenceError::Unsupported
                | InternalEvidenceError::Unproven
                | InternalEvidenceError::InvalidRequest,
            ) => {
                return ConsignmentReadOutcome::Pending(
                    ConsignmentReadPending::AuthenticatedUidUnavailable,
                );
            }
        };
        let target = match self.client.detail(world, &row.external_mail_id) {
            Ok(target) => target,
            Err(error) => return pending_from_error(error),
        };
        let document = match self.client.get(&target, session.cookie()).await {
            Ok(document) => document,
            Err(error) => {
                return match classify_consignment_read_error(error) {
                    ConsignmentReadClassification::Pending(reason) => {
                        ConsignmentReadOutcome::Pending(reason)
                    }
                    ConsignmentReadClassification::ObservedUnclear => {
                        observed(binding, ConsignmentDetailObservation::unavailable(world, row))
                    }
                };
            }
        };
        let observation =
            crate::clients::mooglemail::parser::parse_consignment_detail(&document, world, row);
        if !observation.sender.is_empty() && !self.allowed_sender(&observation.sender) {
            return ConsignmentReadOutcome::Pending(ConsignmentReadPending::AllowlistDenied);
        }
        if session.check_internal_current(deadline).await.is_err() {
            return ConsignmentReadOutcome::Pending(
                ConsignmentReadPending::TransientReadUnavailable,
            );
        }
        observed(binding, observation)
    }

    /// 为一次已授权的寄售处置准备同一认证会话与写前清单。
    ///
    /// 不变量 4、5：本方法不发出 POST；调用方取得许可后只能把本值交给
    /// [`MoogleMailDeliveryTransport::execute_consignment_disposition`](crate::clients::mooglemail::MoogleMailDeliveryTransport::execute_consignment_disposition)
    /// 的消费方。
    pub async fn prepare_consignment_disposition(
        self: &Arc<Self>,
        request: DispositionRequest,
        deadline: Instant,
    ) -> Result<PreparedConsignmentDisposition, NotSentReason> {
        if !request.validate() {
            return Err(NotSentReason::RequestInvalid);
        }
        if deadline <= Instant::now() {
            return Err(NotSentReason::DeadlineExpired);
        }
        let world = request.target().frozen.world;
        let operation = disposition_operation(request.action());
        let session = self
            .credentials
            .begin_exclusive(world, operation)
            .await
            .map_err(map_disposition_read_error)?;
        let binding = self
            .read_identity_in_session(&session, world, deadline)
            .await
            .map_err(map_identity_error)?;
        let (observation, _action_state, _target) =
            read_disposition_page(self, &session, &request).await?;
        if !manifest_matches(&request.target().frozen, &observation) {
            return Err(NotSentReason::ManifestChanged);
        }
        preflight_action(&request, &observation, &binding)?;
        session.check_internal_current(deadline).await.map_err(map_identity_error)?;
        Ok(PreparedConsignmentDisposition {
            gateway: Arc::clone(self),
            session,
            request,
            binding,
            observed_manifest_digest: observation.manifest_digest(),
            deadline,
        })
    }
}

/// 已准备但尚未起写的寄售处置；只在内存中持有认证会话。
pub struct PreparedConsignmentDisposition {
    gateway: Arc<MoogleMailGateway>,
    session: MoogleMailCredentialSession,
    request: DispositionRequest,
    binding: MailAccountEvidenceBinding,
    observed_manifest_digest: [u8; 32],
    deadline: Instant,
}

impl fmt::Debug for PreparedConsignmentDisposition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedConsignmentDisposition")
            .field("action", &self.request.action())
            .field("binding", &self.binding)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl PreparedConsignmentDisposition {
    /// 会话绑定摘要；交给 `start_disposition_write`。
    pub fn session_binding_digest(&self) -> [u8; 32] {
        self.binding.digest
    }

    /// 本次写前实际读到的清单摘要；交给应用层与冻结清单比对。
    pub fn observed_manifest_digest(&self) -> [u8; 32] {
        self.observed_manifest_digest
    }

    /// 原始请求。
    pub fn request(&self) -> &DispositionRequest {
        &self.request
    }

    pub(crate) fn client(&self) -> &crate::clients::mooglemail::MoogleMailClient {
        &self.gateway.client
    }

    pub(crate) fn parser(&self) -> &crate::clients::mooglemail::MoogleMailParser {
        &self.gateway.parser
    }

    pub(crate) fn session(&self) -> &MoogleMailCredentialSession {
        &self.session
    }

    /// 在 POST 之前用同一会话重新读取并逐项比较；任何变化返回确定未发出。
    pub(crate) async fn revalidate(
        &self,
    ) -> Result<RevalidatedConsignmentDisposition, NotSentReason> {
        if self.deadline <= Instant::now() {
            return Err(NotSentReason::DeadlineExpired);
        }
        let world = self.request.target().frozen.world;
        let binding = self
            .gateway
            .read_identity_in_session(&self.session, world, self.deadline)
            .await
            .map_err(map_identity_error)?;
        if !same_binding(&self.binding, &binding) {
            return Err(NotSentReason::IdentityChanged);
        }
        let (observation, action_state, target) =
            read_disposition_page(&self.gateway, &self.session, &self.request).await?;
        if !manifest_matches(&self.request.target().frozen, &observation) {
            return Err(NotSentReason::ManifestChanged);
        }
        preflight_action(&self.request, &observation, &binding)?;
        self.session.check_internal_current(self.deadline).await.map_err(map_identity_error)?;
        Ok(RevalidatedConsignmentDisposition { observation, action_state, target, binding })
    }
}

/// 写前重读结果；只交给同一处置执行器消费。
pub(crate) struct RevalidatedConsignmentDisposition {
    observation: ConsignmentDetailObservation,
    action_state: MoogleMailActionState,
    target: MoogleMailTarget,
    binding: MailAccountEvidenceBinding,
}

impl RevalidatedConsignmentDisposition {
    pub(crate) fn observation(&self) -> &ConsignmentDetailObservation {
        &self.observation
    }

    pub(crate) fn action_state(&self) -> &MoogleMailActionState {
        &self.action_state
    }

    pub(crate) fn target(&self) -> &MoogleMailTarget {
        &self.target
    }

    pub(crate) fn binding(&self) -> &MailAccountEvidenceBinding {
        &self.binding
    }
}

async fn read_disposition_page(
    gateway: &MoogleMailGateway,
    session: &MoogleMailCredentialSession,
    request: &DispositionRequest,
) -> Result<(ConsignmentDetailObservation, MoogleMailActionState, MoogleMailTarget), NotSentReason>
{
    let frozen = &request.target().frozen;
    let world = frozen.world;
    let target = gateway
        .client
        .detail(world, &frozen.external_mail_id)
        .map_err(map_disposition_read_error)?;
    let document =
        gateway.client.get(&target, session.cookie()).await.map_err(map_disposition_read_error)?;
    let row = ConsignmentListRow {
        external_mail_id: frozen.external_mail_id.clone(),
        world,
        world_marker: Some(world),
        sender: frozen.sender.clone(),
        subject: frozen.subject.clone(),
        received_minute_raw: frozen.received_minute_raw.clone(),
        received_at: frozen.received_at,
    };
    let observation =
        crate::clients::mooglemail::parser::parse_consignment_detail(&document, world, &row);
    let action_state =
        gateway.parser.parse_action_state(&document, world).map_err(action_unavailable)?;
    Ok((observation, action_state, target))
}

fn preflight_action(
    request: &DispositionRequest,
    observation: &ConsignmentDetailObservation,
    binding: &MailAccountEvidenceBinding,
) -> Result<(), NotSentReason> {
    if observation.recipient != binding.player_name
        || MoogleMailWorld::from(binding.world) != observation.world
    {
        return Err(NotSentReason::IdentityChanged);
    }
    match request.action() {
        DispositionAction::Take => {
            if !observation.claimable
                || observation.mail_state != Some(1)
                || observation.unknown_attachment
            {
                return Err(NotSentReason::ActionUnavailable);
            }
        }
        DispositionAction::Return => {
            if !observation.returnable {
                return Err(NotSentReason::ActionUnavailable);
            }
            let Some(expected) = request.expected_return() else {
                return Err(NotSentReason::RequestInvalid);
            };
            if expected.frozen() != &request.target().frozen {
                return Err(NotSentReason::ManifestChanged);
            }
        }
    }
    Ok(())
}

fn same_binding(
    expected: &MailAccountEvidenceBinding,
    actual: &MailAccountEvidenceBinding,
) -> bool {
    expected.same_account(actual)
        && expected.world == actual.world
        && expected.authenticated_uid == actual.authenticated_uid
        && expected.player_name == actual.player_name
}

fn disposition_operation(action: DispositionAction) -> MoogleMailOperation {
    match action {
        DispositionAction::Take => MoogleMailOperation::Take,
        DispositionAction::Return => MoogleMailOperation::ReturnMessage,
    }
}

fn map_identity_error(error: InternalEvidenceError) -> NotSentReason {
    match error {
        InternalEvidenceError::Unavailable | InternalEvidenceError::Expired => {
            NotSentReason::TemporaryUnavailable
        }
        InternalEvidenceError::Revoked => NotSentReason::IdentityChanged,
        InternalEvidenceError::Unsupported
        | InternalEvidenceError::Unproven
        | InternalEvidenceError::InvalidRequest => NotSentReason::IdentityChanged,
    }
}

fn map_disposition_read_error(error: crate::clients::mooglemail::MoogleMailError) -> NotSentReason {
    match error.category {
        crate::clients::mooglemail::MoogleMailErrorCategory::RateLimited
        | crate::clients::mooglemail::MoogleMailErrorCategory::AccountBusy
        | crate::clients::mooglemail::MoogleMailErrorCategory::RemoteTemporary
        | crate::clients::mooglemail::MoogleMailErrorCategory::SessionExpired
        | crate::clients::mooglemail::MoogleMailErrorCategory::CredentialUnavailable => {
            NotSentReason::TemporaryUnavailable
        }
        _ => NotSentReason::RequestInvalid,
    }
}

fn action_unavailable(_: crate::clients::mooglemail::MoogleMailError) -> NotSentReason {
    NotSentReason::ActionUnavailable
}

fn observed(
    binding: MailAccountEvidenceBinding,
    observation: ConsignmentDetailObservation,
) -> ConsignmentReadOutcome {
    let observed_at = Timestamp::now();
    let remote_observation_id = remote_observation_id(&binding, &observation);
    ConsignmentReadOutcome::Observed(Box::new(BoundConsignmentObservation {
        observation,
        binding,
        remote_observation_id,
        observed_at,
    }))
}

pub(super) fn pending_from_error(
    error: crate::clients::mooglemail::MoogleMailError,
) -> ConsignmentReadOutcome {
    ConsignmentReadOutcome::Pending(pending_reason(error))
}

pub(super) fn pending_reason(
    error: crate::clients::mooglemail::MoogleMailError,
) -> ConsignmentReadPending {
    match classify_consignment_read_error(error) {
        ConsignmentReadClassification::Pending(reason) => reason,
        ConsignmentReadClassification::ObservedUnclear => {
            ConsignmentReadPending::TransientReadUnavailable
        }
    }
}

pub(super) fn remote_observation_id(
    binding: &MailAccountEvidenceBinding,
    observation: &ConsignmentDetailObservation,
) -> Uuid {
    let mut input = Vec::new();
    input.extend_from_slice(b"auction/consignment/mail-observation/v1\0");
    write_text(&mut input, &binding.authenticated_uid);
    write_text(&mut input, binding.world.as_str());
    write_text(&mut input, &observation.external_mail_id);
    input.extend_from_slice(&observation.manifest_digest());
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &input)
}

fn write_text(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}
