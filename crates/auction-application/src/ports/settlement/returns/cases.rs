//! 结算退件案件的 Admin 读模型、处理命令、类型化错误与存储端口。
//!
//! 案件以入站退件证据唯一（案件 ID 即退件邮件 ID），包括 `untrusted`/`unbound` 隔离记录与
//! `fulfillment_terminal` 记录。命令按 `command_id` 幂等：同 ID 同请求返回原结果，同 ID 不同请求
//! 冲突；陈旧的案件版本失败关闭。资产、金额与收件人只来自服务端冻结事实，请求不携带任意载荷。

use auction_domain::{
    CommandId, SettlementObligationKind, SettlementReturnAction, SettlementReturnBlocker,
    SettlementReturnCaseStatus, SettlementReturnPlan, SettlementReturnTakeUnknownReason,
};
use auction_time::Timestamp;
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use super::evidence::{SettlementReturnMatch, SettlementReturnReason};
use crate::errors::ApplicationError;
use crate::ports::core::{ApplicationFuture, RequestHash};

/// 列表单页上限，与其他 Admin 列表一致。
pub const MAX_SETTLEMENT_RETURN_PAGE_SIZE: i64 = 200;

/// 列表 offset 上限，避免通过无限 offset 扫描整张案件表。
pub const MAX_SETTLEMENT_RETURN_OFFSET: i64 = 10_000;

/// Admin actor 标识的长度上限。
const MAX_ACTOR_BYTES: usize = 128;

/// 退件资产当前位置闭集；详情与资产对账使用同一组名称，互斥且不重复计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettlementReturnAssetLocation {
    /// 来源或绑定未证明：资产（如有）不属于任何已证明的责任。
    Unverified,
    /// 已绑定，资产仍在 Bot 收件箱（退回待领取，含留箱结案）。
    ReturnedInInbox,
    /// Take 已提交，资产仍在收件箱。
    TakeInProgress,
    /// Take 结果未知：位置未知，等待系统可验证证明。
    LocationUnknown,
    /// 已领回，等待补寄。
    TakenAwaitingResend,
    /// 已领回，替原权利人代管。
    TakenInCustody,
    /// 已补寄，新派发外发在途。
    ResentInTransit,
}

impl SettlementReturnAssetLocation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::ReturnedInInbox => "returned_in_inbox",
            Self::TakeInProgress => "take_in_progress",
            Self::LocationUnknown => "location_unknown",
            Self::TakenAwaitingResend => "taken_awaiting_resend",
            Self::TakenInCustody => "taken_in_custody",
            Self::ResentInTransit => "resent_in_transit",
        }
    }

    /// 由案件状态与绑定推出资产位置；同一事实只落在一个位置。
    pub const fn of(status: SettlementReturnCaseStatus, bound: bool) -> Self {
        use SettlementReturnCaseStatus as S;

        if !bound {
            return Self::Unverified;
        }
        match status {
            S::Quarantined | S::RecheckPending | S::Reviewed => Self::Unverified,
            S::AwaitingDecision | S::InboxClosed => Self::ReturnedInInbox,
            S::TakePending => Self::TakeInProgress,
            S::TakeUnknown => Self::LocationUnknown,
            S::ResendPending => Self::TakenAwaitingResend,
            S::Held => Self::TakenInCustody,
            S::Resent => Self::ResentInTransit,
        }
    }
}

/// 案件列表筛选；全部为精确值，分页有界。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettlementReturnListRequest {
    /// `main` / `isekai`。
    pub world: Option<String>,
    /// 原分类（`settlement_return_evidence.match_state`）。
    pub match_state: Option<SettlementReturnMatch>,
    pub status: Option<SettlementReturnCaseStatus>,
    pub obligation_kind: Option<SettlementObligationKind>,
    /// 只列出仍需处理的案件。
    pub open_only: bool,
    /// 收件时间下界（含）。
    pub received_from: Option<Timestamp>,
    /// 收件时间上界（含）。
    pub received_to: Option<Timestamp>,
    /// 参考编号：案件（退件邮件）ID、义务 ID 或原派发投递 ID。
    pub reference: Option<Uuid>,
    pub limit: i64,
    pub offset: i64,
}

impl SettlementReturnListRequest {
    /// 校验有界查询与闭集筛选。
    pub fn validate(&self) -> bool {
        (1..=MAX_SETTLEMENT_RETURN_PAGE_SIZE).contains(&self.limit)
            && (0..=MAX_SETTLEMENT_RETURN_OFFSET).contains(&self.offset)
            && self.world.as_deref().is_none_or(|world| matches!(world, "main" | "isekai"))
            && self.reference.is_none_or(|reference| !reference.is_nil())
            && self.received_from.zip(self.received_to).is_none_or(|(from, to)| from <= to)
    }
}

/// 一个案件的安全摘要；不含正文、发件人原文、Cookie 或装备 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnCaseSummary {
    /// 案件 ID，即退件入站邮件 ID。
    pub return_mail_id: Uuid,
    pub case_version: i64,
    pub status: SettlementReturnCaseStatus,
    pub match_state: SettlementReturnMatch,
    pub reason: SettlementReturnReason,
    pub world_id: String,
    /// 退件在 Bot 收件箱中的远端 MID。
    pub external_mail_id: String,
    pub received_at: Timestamp,
    pub obligation_id: Option<Uuid>,
    pub obligation_kind: Option<SettlementObligationKind>,
    pub auction_id: Option<Uuid>,
    pub lot_id: Option<Uuid>,
    /// 被退回的原派发投递；只在已绑定时存在。
    pub delivery_id: Option<Uuid>,
    pub dispatch_no: Option<u32>,
    /// 原派发冻结收件人；补寄只寄给这个名字。
    pub recipient: Option<String>,
    pub credits: i64,
    pub cod: i64,
    pub attachment_count: u32,
    pub plan: Option<SettlementReturnPlan>,
    /// 案件建立时冻结的补寄上限（不含首寄）。
    pub resend_limit: u8,
    /// 原义务当前已发生的补寄次数。
    pub resends_used: Option<u32>,
    pub asset_location: SettlementReturnAssetLocation,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

/// 分页结果；`total` 与 `items` 使用同一筛选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnPage {
    pub items: Vec<SettlementReturnCaseSummary>,
    pub total: i64,
}

/// 案件处理事件（只追加）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnEvent {
    pub case_version: i64,
    /// 事件种类闭集见 `settlement_return_events.event_kind`。
    pub event_kind: String,
    pub occurred_at: Timestamp,
    /// Admin 命令触发时为 actor；Worker 事件为空。
    pub actor_id: Option<String>,
    pub command_id: Option<CommandId>,
    /// 安全细节（状态、原因码、派发编号等），不含正文或秘密。
    pub details: serde_json::Value,
}

/// 原义务的一次派发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnDispatch {
    pub dispatch_no: u32,
    pub delivery_id: Uuid,
    /// 投递当前状态；补寄已授权但尚未准备时为空。
    pub delivery_status: Option<String>,
    pub sending_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    /// 授权这次补寄的退件案件；首寄为空。
    pub authorized_by: Option<Uuid>,
}

/// 本案的持久 Take。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnTake {
    pub execution_state: String,
    pub delivery_id: Option<Uuid>,
    pub delivery_status: Option<String>,
    pub failure_code: Option<String>,
    /// 领回证明成立的时刻。
    pub proven_at: Option<Timestamp>,
}

/// 代管责任：原权利人与代管资产。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnCustody {
    /// 原派发冻结收件人（原权利人）。
    pub beneficiary: String,
    pub credits: i64,
    pub equipment_ids: Vec<String>,
}

/// 案件详情；允许动作与阻断原因由同一准入规则计算，服务端仍在命令事务内复核。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnDetail {
    pub summary: SettlementReturnCaseSummary,
    pub events: Vec<SettlementReturnEvent>,
    pub dispatches: Vec<SettlementReturnDispatch>,
    pub take: Option<SettlementReturnTake>,
    /// 本案授权的补寄派发编号。
    pub resend_dispatch_no: Option<u32>,
    pub custody: Option<SettlementReturnCustody>,
    pub allowed_actions: Vec<SettlementReturnAction>,
    /// 不允许的动作及其类型化原因，按动作闭集顺序。
    pub blockers: Vec<(SettlementReturnAction, SettlementReturnBlocker)>,
    /// 案件等待只读重新核验，但没有在线 Worker 声明能读取该世界的 MoogleMail（未启用邮件、该世界
    /// 未配置或 Worker 不在线）：核验能力不可用，案件保持待核验，能力恢复后由 Worker 自动继续。
    pub recheck_unavailable: bool,
    /// 案件待领回、Take 尚未执行，但没有 Worker 能以持久凭据身份执行该世界的 Take（只配置 Cookie
    /// 或未启用邮件）：领回能力不可用，案件停在待领回，能力恢复后由 Worker 自动继续。
    pub take_unavailable: bool,
    /// Take 结果未知（`take_unknown`）时的具体原因：最近一次原会话观察，没有观察时由持久事实
    /// 推出；其他状态为空。只读说明，再次领回的资格仍由命令事务锁内复核。
    pub take_unknown_reason: Option<SettlementReturnTakeUnknownReason>,
}

/// Admin 处理命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnCommand {
    pub command_id: CommandId,
    pub request_hash: RequestHash,
    pub return_mail_id: Uuid,
    /// 页面读取时的案件版本。
    pub expected_version: i64,
    pub action: SettlementReturnAction,
    /// 服务端会话身份，不来自请求体。
    pub actor_id: String,
    pub requested_at: Timestamp,
}

/// 进入请求摘要的稳定字段；字段顺序决定摘要字节，不可调整。
#[derive(Serialize)]
struct SettlementReturnCommandDigest<'a> {
    return_mail_id: Uuid,
    expected_version: i64,
    action: &'a str,
    actor_id: &'a str,
}

impl SettlementReturnCommand {
    /// 构造并摘要命令；身份、版本或 actor 不合法时返回 [`SettlementReturnError::Invalid`]，
    /// 摘要编码失败原样返回序列化错误。
    pub fn new(
        command_id: CommandId,
        return_mail_id: Uuid,
        expected_version: i64,
        action: SettlementReturnAction,
        actor_id: &str,
        requested_at: Timestamp,
    ) -> Result<Self, ApplicationError> {
        if command_id.is_nil()
            || return_mail_id.is_nil()
            || expected_version <= 0
            || actor_id.is_empty()
            || actor_id != actor_id.trim()
            || actor_id.len() > MAX_ACTOR_BYTES
            || actor_id.chars().any(char::is_control)
        {
            return Err(SettlementReturnError::Invalid.into());
        }
        let request_hash = RequestHash::from_request(
            "settlement_return_command",
            command_id.as_uuid(),
            &SettlementReturnCommandDigest {
                return_mail_id,
                expected_version,
                action: action.as_str(),
                actor_id,
            },
        )?;
        Ok(Self {
            command_id,
            request_hash,
            return_mail_id,
            expected_version,
            action,
            actor_id: actor_id.to_owned(),
            requested_at,
        })
    }
}

/// 命令结果；同 ID 重放返回首次提交时的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnCommandResult {
    pub command_id: CommandId,
    pub return_mail_id: Uuid,
    pub action: SettlementReturnAction,
    /// 命令提交后的案件版本。
    pub case_version: i64,
    pub status: SettlementReturnCaseStatus,
}

/// 结算退件 Admin 读写的稳定业务错误；只携带闭集分类。HTTP 映射：`Invalid` 400，`NotFound`
/// 404，`Unavailable`/`StoredResultCorrupt` 503，其余 409。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SettlementReturnError {
    #[error("结算退件命令参数无效")]
    Invalid,
    #[error("结算退件案件不存在")]
    NotFound,
    #[error("结算退件案件版本已变化")]
    Stale,
    #[error("结算退件命令与既有命令冲突")]
    CommandConflict,
    #[error("结算退件来源未证明，只能标记已核对")]
    Untrusted,
    #[error("结算退件未绑定原派发，只能标记已核对或重新核验")]
    Unbound,
    #[error("退回资产与原派发冻结资产不符")]
    AssetsMismatch,
    #[error("结算退件当前状态不允许该动作")]
    ActionConflict,
    #[error("缺少已持久化的领回证明")]
    ProofRequired,
    #[error("补寄次数已达案件冻结上限")]
    ResendLimit,
    #[error("原结算义务已不可补寄")]
    ObligationIneligible,
    #[error("原派发的发送结果未核实，须先经 Sent 证明收口原发送")]
    SendUnresolved,
    #[error("再次领回需要最近一次观察证明原资产完整仍在且可领取")]
    ObservationRequired,
    #[error("结算退件持久化结果损坏")]
    StoredResultCorrupt,
    #[error("结算退件依赖暂不可用")]
    Unavailable,
}

impl From<SettlementReturnBlocker> for SettlementReturnError {
    fn from(blocker: SettlementReturnBlocker) -> Self {
        match blocker {
            SettlementReturnBlocker::Untrusted => Self::Untrusted,
            SettlementReturnBlocker::Unbound => Self::Unbound,
            SettlementReturnBlocker::AssetsMismatch => Self::AssetsMismatch,
            SettlementReturnBlocker::StateConflict => Self::ActionConflict,
            SettlementReturnBlocker::ResendLimit => Self::ResendLimit,
            SettlementReturnBlocker::ObligationIneligible => Self::ObligationIneligible,
            SettlementReturnBlocker::ProofRequired => Self::ProofRequired,
            SettlementReturnBlocker::SendUnresolved => Self::SendUnresolved,
            SettlementReturnBlocker::ObservationRequired => Self::ObservationRequired,
        }
    }
}

/// Admin 结算退件端口；错误以 [`crate::core::ApplicationError::SettlementReturn`] 携带类型化分类。
pub trait SettlementReturnAdminPort: Send + Sync {
    fn list_settlement_returns(
        &self,
        request: SettlementReturnListRequest,
    ) -> ApplicationFuture<'_, SettlementReturnPage>;

    /// 读取单个案件；不存在时返回 [`SettlementReturnError::NotFound`]。
    fn settlement_return_detail(
        &self,
        return_mail_id: Uuid,
    ) -> ApplicationFuture<'_, SettlementReturnDetail>;

    /// 执行处理命令：同事务写入命令账本、案件投影、事件与（领回时）持久 Take；不触发任何外部写入。
    fn handle_settlement_return(
        &self,
        command: SettlementReturnCommand,
    ) -> ApplicationFuture<'_, SettlementReturnCommandResult>;

    /// 仍需处理的案件数（导航待办计数）。
    fn open_settlement_return_count(&self) -> ApplicationFuture<'_, i64>;
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{SettlementReturnAssetLocation, SettlementReturnCommand, SettlementReturnError};

    /// 同一命令字段得到同一摘要；动作、版本或 actor 不同都改变摘要。
    #[test]
    fn command_digest_binds_action_version_and_actor() {
        let command_id = CommandId::from_uuid(Uuid::from_u128(1));
        let case = Uuid::from_u128(2);
        let now = Timestamp::now();
        let build = |version, action, actor: &str| {
            SettlementReturnCommand::new(command_id, case, version, action, actor, now)
                .map(|command| command.request_hash)
                .ok()
        };
        let base = build(3, SettlementReturnAction::ReclaimAndHold, "admin");
        assert!(base.is_some());
        assert_eq!(base, build(3, SettlementReturnAction::ReclaimAndHold, "admin"));
        assert_ne!(base, build(4, SettlementReturnAction::ReclaimAndHold, "admin"));
        assert_ne!(base, build(3, SettlementReturnAction::CloseInbox, "admin"));
        assert_ne!(base, build(3, SettlementReturnAction::ReclaimAndHold, "other"));
        for (version, actor) in [(0, "admin"), (1, " admin"), (1, "")] {
            assert_matches!(
                SettlementReturnCommand::new(
                    command_id,
                    case,
                    version,
                    SettlementReturnAction::Acknowledge,
                    actor,
                    now
                ),
                Err(crate::errors::ApplicationError::SettlementReturn(
                    SettlementReturnError::Invalid
                ))
            );
        }
    }

    /// 资产位置互斥：未绑定一律未核验，已绑定按状态落在唯一位置。
    #[test]
    fn asset_location_follows_status_and_binding() {
        for status in SettlementReturnCaseStatus::ALL {
            assert_eq!(
                SettlementReturnAssetLocation::of(*status, false),
                SettlementReturnAssetLocation::Unverified
            );
        }
        assert_eq!(
            SettlementReturnAssetLocation::of(SettlementReturnCaseStatus::Held, true),
            SettlementReturnAssetLocation::TakenInCustody
        );
        assert_eq!(
            SettlementReturnAssetLocation::of(SettlementReturnCaseStatus::TakeUnknown, true),
            SettlementReturnAssetLocation::LocationUnknown
        );
    }
}
