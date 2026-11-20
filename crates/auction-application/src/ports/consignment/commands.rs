//! 六类写入的规范输入、事务边界和不含秘密的回执。

use std::fmt;

use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingPublicationTtls,
};
use auction_domain::consignment::{ListingSnapshot, ListingState, ListingVersion};
use auction_domain::{CommandId, EquipmentId, EquipmentKey, LotId, UserId, valid_text};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::admin::EquipmentKeySealerPort;
use crate::ports::consignment::equipment::{
    LookupObservation, LookupTransientReason, RegistrationReplayCrypto, contains_equipment_link,
};
use crate::ports::consignment::errors::{
    ConsignmentError, ConsignmentFuture, ListingBusinessRejection, UnrecordedListingRejection,
};
use crate::ports::consignment::reads::ListingWorlds;
use crate::ports::consignment::work::LookupFence;
use crate::ports::core::RequestHash;

// 规范化后的语义字段由账本 owner 定序编码。不变量 1、7：登记使用原逻辑比较钥的 MAC，
// 其他操作用版本化 SHA-256；派生 UUID、时钟、运行配置与加密随机数不属于登记请求摘要。

/// 寄售命令账本中的稳定操作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingOperation {
    /// 新登记。
    Register,
    /// 卖家改价。
    Reprice,
    /// 卖家取消。
    Withdraw,
    /// 提交一次解析观察。
    ApplyLookup,
    /// 自动清退。
    AutoClose,
    /// 管理员下架。
    AdminDelist,
}

impl ListingOperation {
    /// 持久账本、摘要及安全回执使用的稳定操作码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Reprice => "reprice",
            Self::Withdraw => "withdraw",
            Self::ApplyLookup => "apply_lookup",
            Self::AutoClose => "auto_close",
            Self::AdminDelist => "admin_delist",
        }
    }
}

/// 经 URL 校验后用于登记仲裁的字段。凭证只借用到本次命令结束。
#[derive(Debug)]
pub struct RegistrationInput<'a> {
    /// 规范世界和 EID。
    pub identity: &'a ListingEquipmentIdentity,
    /// 已规范化的读取凭证。
    pub key: &'a EquipmentKey,
    /// 不可变的登记模式。
    pub mode: ListingMode,
    /// 整数标价。
    pub price: ListingPrice,
    /// 卖家确认过的费率。
    pub expected_fee_bps: FeeBps,
}

/// 首次登记的准入策略，由本次服务装配提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingRegistrationPolicy {
    /// 本地配置开关。首次受理还需核对持久准入与就绪。
    pub enabled: bool,
    /// 服务配置允许登记的世界，须另核数据库的 world 状态。
    pub worlds: ListingWorlds,
    /// 非寄售固定零费率，此字段只适用于寄售。
    pub consigned_fee_bps: FeeBps,
    /// 两世界共用的有效活动名额。
    pub max_open_per_user: u8,
    /// 数据库时钟滚动一小时的登记额度。
    pub registrations_per_hour: u8,
}

/// 登记命令及首次执行候选 ID。重放必须比较固定目标 `new`。
#[derive(Debug)]
pub struct RegisterListingCommand<'a> {
    /// 整个寄售账本内唯一的命令 ID。
    pub command_id: CommandId,
    /// 从当前会话取得的卖家。
    pub seller_user_id: UserId,
    /// 参与重放比较的输入。
    pub input: RegistrationInput<'a>,
    /// 只供首次成功使用的挂单 UUID。
    pub listing_id: Uuid,
    /// 只供首次成功使用的解析工作 UUID。
    pub work_id: Uuid,
    /// 登记信封终身固定的内部 AAD UUID。
    pub aad_equipment_id: EquipmentId,
    /// 首次需要建立逻辑比较钥时使用的候选 UUID。
    pub candidate_replay_key_id: Uuid,
    /// 首次执行的准入政策。
    pub policy: ListingRegistrationPolicy,
}

/// 已认证卖家对现有挂单发出的版本化命令目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SellerListingTarget {
    /// 整个寄售账本内唯一的命令 ID。
    pub command_id: CommandId,
    /// 唯一授权读取该命令或触碰目标的会话用户。
    pub seller_user_id: UserId,
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 客户端最后确认的已提交版本。
    pub expected_version: ListingVersion,
}

/// 本人改价，配置关闭时只允许重放已受理的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepriceListingCommand {
    /// 目标、预期版本和标价的规范摘要，不含本地开关。
    pub request_hash: RequestHash,
    /// 命令身份和版本。
    pub target: SellerListingTarget,
    /// 确认的新标价。
    pub price: ListingPrice,
    /// 本地配置开关，仅用于首次受理。
    pub enabled: bool,
}

/// 本人取消，关闭后仍可触发强制清退。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawListingCommand {
    /// 目标与预期版本的规范摘要。
    pub request_hash: RequestHash,
    /// 命令身份和版本。
    pub target: SellerListingTarget,
}

/// 运营私有说明，只进入 Admin 私有视图，永不进入公开 DTO。
///
/// 筛查限于可识别的装备链接、裸 10 位十六进制和 43 位登录 key 形态。
/// 任意格式的旧版手工登录 key 可能与普通说明同形，无法从自由文本可靠识别。
/// 调试输出隐去原文。
#[derive(Clone, PartialEq, Eq)]
pub struct ListingDelistReason(String);

impl ListingDelistReason {
    /// 校验 1–256 UTF-8 字节的规范单行说明及可识别的凭证形态。
    pub fn new(reason: String) -> Result<Self, ConsignmentError> {
        // 不变量 6、7：合同未定义任意自由文本中的裸凭据识别，保守拒绝链接和常见
        // 裸凭据形态；运营仍不得粘贴其他凭据。错误不带原说明。
        if !valid_text(&reason, 256)
            || reason.contains(['\u{2028}', '\u{2029}'])
            || contains_equipment_link(&reason)
            || reason
                .split(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_'))
                .any(|part| part.len() == 43)
            || reason
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|part| part.len() == 10 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(ConsignmentError::InvalidReason);
        }
        Ok(Self(reason))
    }

    /// 借用私有说明，供持久化和已认证 Admin 映射使用。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ListingDelistReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 已认证管理员的下架命令。账本绑定固定的 `admin_actor`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDelistListingCommand {
    /// 已认证 Admin 会话的固定运营身份，须参与全局命令仲裁。
    pub admin_actor: String,
    /// 目标、预期版本和私有说明的规范摘要。不记录说明原文到诊断。
    pub request_hash: RequestHash,
    /// 整个寄售账本内唯一的命令 ID。
    pub command_id: CommandId,
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 运营确认的已提交版本。
    pub expected_version: ListingVersion,
    /// 私有运营说明。
    pub reason: ListingDelistReason,
}

/// 自动清退的证据标识，原因与证据互相约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCloseEvidence {
    /// 已提交且不在登记基线内的拍卖 lot。
    AuctionLotCreated(LotId),
    /// 属于该卖家的拉黑历史事实 UUID。
    SellerIneligible(Uuid),
    /// 已递增的停用代次。
    ServiceDisabled(u64),
    /// 解析时冻结的总截止。
    LookupDeadline(Timestamp),
    /// 发布时冻结的展示截止。
    DisplayExpired(Timestamp),
    /// 发布时冻结的待寄截止。
    DepositExpired(Timestamp),
}

/// 维护扫描获得的候选。写入事务必须重查证据与强制终止优先序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCloseCandidate {
    /// 目标挂单。
    pub listing_id: Uuid,
    /// 唤醒本次清退的证据。
    pub evidence: AutoCloseEvidence,
}

/// 系统清退命令。ID 稳定地绑定候选的挂单、原因和真实证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCloseListingCommand {
    /// 目标与清退证据的规范摘要。
    pub request_hash: RequestHash,
    /// 按规范证据派生的稳定命令 ID。
    pub command_id: CommandId,
    /// 目标及证据。
    pub candidate: AutoCloseCandidate,
}

/// 解析结果提交。围栏代替预期挂单版本，采用锁内最新标价。
#[derive(Debug)]
pub struct ApplyListingLookupCommand {
    /// 目标、工作围栏及规范观察的摘要，不含采集时刻。
    pub request_hash: RequestHash,
    /// 由 `(work_id,generation,fence,"result")` 稳定派生。
    pub command_id: CommandId,
    /// 领取时提交的工作身份和租约凭证。
    pub fence: LookupFence,
    /// 规范观察或闭集错误。摘要不包含 Worker 名或采集时刻。
    pub observation: LookupObservation,
    /// 首次发布时冻结的展示和待寄期限，不属于重放比较。
    pub publication_ttls: ListingPublicationTtls,
}

/// 账本可重放的唯一初次决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredListingDecision<T> {
    /// 成功回执或工作事实。
    Accepted(T),
    /// 已记安全回执与拒绝审计。强制清退仍可能在同次事务提交。
    Rejected(ListingBusinessRejection),
}

/// 首次确定拒绝，可区分应已记账和禁止记账的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingCommandRejection {
    /// 已记账的确定性业务拒绝。
    Recorded(ListingBusinessRejection),
    /// 未产生账本行的准入、仲裁或围栏拒绝。
    Unrecorded(UnrecordedListingRejection),
}

/// 整项仲裁的三类确定结果。未知提交或依赖失败由端口的外层错误返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingCommandOutcome<T> {
    /// 首次接受并提交的事实或无变化回执。
    Accepted(T),
    /// 已提交原决定的原样回放。
    Replayed(StoredListingDecision<T>),
    /// 首次确定性拒绝。
    Rejected(ListingCommandRejection),
}

impl<T> ListingCommandOutcome<T> {
    /// 投影为用例结果，重放拒绝沿用初次安全错误。
    pub fn into_result(self) -> Result<T, ConsignmentError> {
        match self {
            Self::Accepted(receipt) | Self::Replayed(StoredListingDecision::Accepted(receipt)) => {
                Ok(receipt)
            }
            Self::Replayed(StoredListingDecision::Rejected(error))
            | Self::Rejected(ListingCommandRejection::Recorded(error)) => Err(error.into()),
            Self::Rejected(ListingCommandRejection::Unrecorded(error)) => Err(error.into()),
        }
    }
}

/// 202 登记回执，其状态和版本固定为 `resolving`、1。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingRegistered {
    /// 新挂单 UUID。
    listing_id: Uuid,
    /// 登记时标价。
    pub price: ListingPrice,
}

impl ListingRegistered {
    /// 构造登记成功原回执，不变量 1、6：目标必须存在于回执，版本和状态固定。
    pub fn new(listing_id: Uuid, price: ListingPrice) -> Result<Self, ConsignmentError> {
        if listing_id.is_nil() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { listing_id, price })
    }

    /// 新挂单 UUID。
    pub const fn listing_id(self) -> Uuid {
        self.listing_id
    }

    /// 登记时的原始版本。
    pub const fn version(self) -> ListingVersion {
        ListingVersion::FIRST
    }

    /// 登记时的原始状态。
    pub const fn state(self) -> ListingState {
        ListingState::Resolving
    }
}

/// 首次改价结果与同价无变化的区别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingPriceDisposition {
    /// 标价和版本已更新。
    Repriced,
    /// 新命令的标价相同，版本不变。
    Unchanged,
}

/// 200 改价回执，包含原决定时的活动态和标价。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingPriceReceipt {
    /// 目标挂单。
    listing_id: Uuid,
    /// 原回执中的已提交版本。
    pub version: ListingVersion,
    /// 原回执中的状态。
    state: ListingState,
    /// 原回执中的当前标价。
    pub price: ListingPrice,
    /// 是否产生了新版本。
    pub disposition: ListingPriceDisposition,
}

impl ListingPriceReceipt {
    /// 构造改价或同价原回执。不变量 1、6：两个成功处置都只能来自活动态。
    pub fn new(
        listing_id: Uuid,
        version: ListingVersion,
        state: ListingState,
        price: ListingPrice,
        disposition: ListingPriceDisposition,
    ) -> Result<Self, ConsignmentError> {
        if listing_id.is_nil() || !state.is_active() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { listing_id, version, state, price, disposition })
    }

    /// 目标挂单 UUID。
    pub const fn listing_id(self) -> Uuid {
        self.listing_id
    }

    /// 原回执中的活动态。
    pub const fn state(self) -> ListingState {
        self.state
    }
}

/// 200 主动关闭回执，其状态固定为 `closed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingClosed {
    /// 目标挂单。
    listing_id: Uuid,
    /// 关闭时的已提交版本。
    pub version: ListingVersion,
    /// 关闭时的标价，可按 §7 回执规则省略。
    pub price: Option<ListingPrice>,
}

impl ListingClosed {
    /// 构造主动关闭原回执。不变量 1、6：保留原目标和版本，价格可空。
    pub fn new(
        listing_id: Uuid,
        version: ListingVersion,
        price: Option<ListingPrice>,
    ) -> Result<Self, ConsignmentError> {
        if listing_id.is_nil() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { listing_id, version, price })
    }

    /// 目标挂单 UUID。
    pub const fn listing_id(self) -> Uuid {
        self.listing_id
    }

    /// 原回执中的终态。
    pub const fn state(self) -> ListingState {
        ListingState::Closed
    }
}

/// 解析提交已落库的事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingLookupEffect {
    /// 挂单发布或终结后的安全投影。
    ListingChanged(ListingSnapshot),
    /// 本次暂时失败，持久工作重排，挂单版本未变。
    Rescheduled {
        /// 下一次可领的数据库时刻。
        next_attempt_at: Timestamp,
        /// 本次安全分类。
        reason: LookupTransientReason,
    },
}

/// 自动下架已落库的事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingAutoCloseEffect {
    /// 挂单终结后的安全投影。
    Terminated(ListingSnapshot),
    /// 仅补录了真实拍卖碰撞，保留首个终态原因与版本。
    ConflictsRecorded,
    /// 条件已证且持久状态与关联均完整，没有追加挂单事实。
    Unchanged,
}

/// 按已认证本人查询时可返回的三种命令及各自原决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedListingDecision {
    /// 初次登记的 202 或其业务拒绝。
    Register(StoredListingDecision<ListingRegistered>),
    /// 初次改价的 200 或其业务拒绝。
    Reprice(StoredListingDecision<ListingPriceReceipt>),
    /// 初次取消的 200 或其业务拒绝。
    Withdraw(StoredListingDecision<ListingClosed>),
}

impl OwnedListingDecision {
    /// 返回该私人回执的原操作。
    pub const fn operation(self) -> ListingOperation {
        match self {
            Self::Register(_) => ListingOperation::Register,
            Self::Reprice(_) => ListingOperation::Reprice,
            Self::Withdraw(_) => ListingOperation::Withdraw,
        }
    }

    /// 返回该命令初次执行的业务状态和代码，独立于外层 GET 的状态。
    pub const fn status_and_code(self) -> (u16, &'static str) {
        match self {
            Self::Register(StoredListingDecision::Accepted(_)) => (202, "accepted"),
            Self::Reprice(StoredListingDecision::Accepted(_))
            | Self::Withdraw(StoredListingDecision::Accepted(_)) => (200, "ok"),
            Self::Register(StoredListingDecision::Rejected(error))
            | Self::Reprice(StoredListingDecision::Rejected(error))
            | Self::Withdraw(StoredListingDecision::Rejected(error)) => {
                (error.status(), error.code())
            }
        }
    }
}

/// 本人命令查询的安全账本投影，外层读成功独立于初次业务决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnedListingCommandReceipt {
    /// 账本中的原命令 UUID。
    pub command_id: CommandId,
    /// 操作及原业务决定。
    pub decision: OwnedListingDecision,
}

/// 寄售写入事务。不变量 1、2、3、5、6、7、8：先跨目标仲裁，再锁内核验并原子提交
/// 投影、事件、审计、账本及适用的工作与凭证变更。重放不重跑业务，不在持锁期间发网。
pub trait ListingCommandStore: Send + Sync {
    /// 先比较已存登记用的逻辑钥，再检查新登记准入及可用性。只有首次受理才封装和建工作。
    /// sealer 和比较钥接口只能执行本地、有界密码运算。缺 sealer 不影响同请求回放。
    fn register<'a>(
        &'a self,
        command: RegisterListingCommand<'a>,
        sealer: Option<&'a dyn EquipmentKeySealerPort>,
        replay_crypto: Option<&'a dyn RegistrationReplayCrypto>,
    ) -> ConsignmentFuture<'a, ListingCommandOutcome<ListingRegistered>>;

    /// 所有者检查优先于状态；强制终止与原拒绝同提交。改价不改期限或解析围栏。
    fn reprice(
        &self,
        command: RepriceListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingPriceReceipt>>;

    /// 本人取消原子终结、释放名额、失效工作并擦除信封。
    fn withdraw(
        &self,
        command: WithdrawListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>>;

    /// 先重放，再核验工作、挂单、代次、token、fence 与活租期。发布或重排、全局冷却、
    /// 许可释放与账本同提交，失败围栏不得动后续许可。
    fn apply_lookup(
        &self,
        command: ApplyListingLookupCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingLookupEffect>>;

    /// 锁内重证并应用强制终结优先序。首次碰撞记录同快照的完整差集。
    fn auto_close(
        &self,
        command: AutoCloseListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingAutoCloseEffect>>;

    /// 版本匹配且有效活动时立即下架，私有运营说明单独保存。
    fn admin_delist(
        &self,
        command: AdminDelistListingCommand,
    ) -> ConsignmentFuture<'_, ListingCommandOutcome<ListingClosed>>;
}
