//! 全局解析许可、持久工作租约及发送前围栏。

use std::fmt;
use std::time::Duration;

use auction_domain::consignment::{ListingEquipmentIdentity, ListingVersion};
use auction_domain::valid_text;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::equipment::{ListingCredentialError, ListingRegistrationSecret};
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};

/// 解析固定预算，领取即计次。
pub const MAX_LISTING_LOOKUP_ATTEMPTS: u8 = 6;

/// 可重新确认和提交的工作围栏。不变量 5、6、7：代次、fence、token 与持久行匹配，
/// token 不进入调试或读侧投影。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LookupFence {
    work_id: Uuid,
    listing_id: Uuid,
    generation: u64,
    fence: i64,
    token: Uuid,
}

impl LookupFence {
    /// 校验领取凭证的非零身份、代次及正围栏。损坏的持久形状视为依赖不可用。
    pub fn new(
        work_id: Uuid,
        listing_id: Uuid,
        generation: u64,
        fence: i64,
        token: Uuid,
    ) -> Result<Self, ConsignmentError> {
        if work_id.is_nil()
            || listing_id.is_nil()
            || !(1..=ListingVersion::MAX).contains(&generation)
            || fence <= 0
            || token.is_nil()
        {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { work_id, listing_id, generation, fence, token })
    }

    /// 工作 UUID。
    pub const fn work_id(self) -> Uuid {
        self.work_id
    }

    /// 挂单 UUID。
    pub const fn listing_id(self) -> Uuid {
        self.listing_id
    }

    /// 工作代次。
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// 领取围栏，只在数据库条件和内部命令身份中使用。
    pub const fn fence(self) -> i64 {
        self.fence
    }

    /// 仅供存储执行租约 CAS 的私有 token。
    pub const fn token(self) -> Uuid {
        self.token
    }
}

impl fmt::Debug for LookupFence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LookupFence")
            .field("work_id", &self.work_id)
            .finish_non_exhaustive()
    }
}

/// 已验证的领取预算，由 Worker 装配提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingLookupBudget {
    timeout: Duration,
    min_interval: Duration,
}

impl ListingLookupBudget {
    /// 校验 §6 的整秒超时和整数毫秒间隔。
    pub fn new(timeout_seconds: u64, min_interval_ms: u64) -> Result<Self, ConsignmentError> {
        if !(1..=30).contains(&timeout_seconds) || !(250..=60_000).contains(&min_interval_ms) {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self {
            timeout: Duration::from_secs(timeout_seconds),
            min_interval: Duration::from_millis(min_interval_ms),
        })
    }

    /// 从领取的数据库时刻计算的网络硬预算。
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// 全局连续两次发放许可的起始间隔。
    pub const fn min_interval(self) -> Duration {
        self.min_interval
    }
}

/// 本次尝试待提交的领取参数。运行实例与 token 禁止进入诊断。
#[derive(Clone, PartialEq, Eq)]
pub struct LookupClaimRequest {
    service_epoch: u64,
    worker: String,
    token: Uuid,
    budget: ListingLookupBudget,
}

impl LookupClaimRequest {
    /// 检查运行实例、代次和新 token 的形状。暂停时调用方不得申请新领取。
    pub fn new(
        service_epoch: u64,
        worker: String,
        token: Uuid,
        budget: ListingLookupBudget,
    ) -> Result<Self, ConsignmentError> {
        if !(1..=ListingVersion::MAX).contains(&service_epoch)
            || !valid_text(&worker, 256)
            || token.is_nil()
        {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { service_epoch, worker, token, budget })
    }

    /// Worker 已同步的代次。
    pub const fn service_epoch(&self) -> u64 {
        self.service_epoch
    }

    /// 本地持久租约的实例名。
    pub fn worker(&self) -> &str {
        &self.worker
    }

    /// 本次新租约的随机 token。
    pub const fn token(&self) -> Uuid {
        self.token
    }

    /// 本次预算。
    pub const fn budget(&self) -> ListingLookupBudget {
        self.budget
    }
}

/// 一次领取已提交，仍需在发网前复查暂停与围栏。
#[derive(Debug)]
pub struct ListingLookupClaim {
    /// 工作、挂单和已提交围栏。
    fence: LookupFence,
    /// 从持久行得到的装备身份。
    identity: ListingEquipmentIdentity,
    /// 本次准入代次。
    service_epoch: u64,
    /// 领取后已提交的尝试次数，范围 1–6。
    attempt: u8,
    /// 本次网络硬截止，取领取预算与挂单总截止的较早值，不随续租延长。
    deadline: Timestamp,
    /// 当前持久租期，初始预留硬截止之外 5 秒。
    lease_until: Timestamp,
    /// 原登记 AAD 与密文。缺失或损坏应在本次围栏内提交凭证失败，禁止发网。
    credential: Result<ListingRegistrationSecret, ListingCredentialError>,
}

impl ListingLookupClaim {
    /// 从已提交的领取事实建立结果。不变量 5、6：领取已计次，准入代次必须有效。
    pub fn new(
        fence: LookupFence,
        identity: ListingEquipmentIdentity,
        service_epoch: u64,
        attempt: u8,
        deadline: Timestamp,
        lease_until: Timestamp,
        credential: Result<ListingRegistrationSecret, ListingCredentialError>,
    ) -> Result<Self, ConsignmentError> {
        if !(1..=MAX_LISTING_LOOKUP_ATTEMPTS).contains(&attempt)
            || !(1..=ListingVersion::MAX).contains(&service_epoch)
        {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { fence, identity, service_epoch, attempt, deadline, lease_until, credential })
    }

    /// 工作、挂单和已提交围栏。
    pub const fn fence(&self) -> LookupFence {
        self.fence
    }

    /// 从持久行得到的装备身份。
    pub const fn identity(&self) -> &ListingEquipmentIdentity {
        &self.identity
    }

    /// 本次准入代次。
    pub const fn service_epoch(&self) -> u64 {
        self.service_epoch
    }

    /// 本次领取后的已提交计次。
    pub const fn attempt(&self) -> u8 {
        self.attempt
    }

    /// 不随续租延长的网络硬截止。
    pub const fn deadline(&self) -> Timestamp {
        self.deadline
    }

    /// 领取时授予的租期。
    pub const fn lease_until(&self) -> Timestamp {
        self.lease_until
    }

    /// 原登记信封。遇凭证错误以本次围栏提交安全分类，禁止发网。
    pub fn credential(&self) -> Result<&ListingRegistrationSecret, ListingCredentialError> {
        self.credential.as_ref().map_err(|error| *error)
    }
}

/// 诊断与调度可读的全局许可时间，不携带持有者凭证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LookupPermitStatus {
    /// 本次只读快照的数据库时刻。
    pub as_of: Timestamp,
    /// 抓取许可所处的持久代次。
    pub service_epoch: u64,
    /// 持久准入开关。
    pub admitting: bool,
    /// 下一起始许可的下界。
    pub next_allowed_at: Timestamp,
    /// 上游限流时叠加的全局冷却。
    pub cooldown_until: Option<Timestamp>,
    /// 现有许可或在途时间栅栏的保留截止。
    pub permit_until: Option<Timestamp>,
}

/// 复查后唯一能授权开始 GET 的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupDispatch {
    /// 围栏、启用代次、资格和硬截止均仍然有效。
    Allowed {
        /// 实际调用必须采用的最早截止。
        deadline: Timestamp,
    },
    /// 外部读取已暂停，未发送时让出执行权且不倒扣领取次数。
    Paused,
    /// 租约、资格、期限或服务代次不再授权发送。
    Lost,
}

/// Worker 放弃领取时提供的远端状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupAbandonment {
    /// 确认本次没有发网，可释放本 permit。
    NotSent,
    /// 已确认本次请求结束，可以释放本 permit。
    RequestFinished,
    /// 可能仍在途，只能作废提交资格，保留全局时间围栏。
    MayBeInFlight,
}

/// 工作租约 CAS 的结果；丢失围栏不得修改新的持有者。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupLeaseUpdate<T> {
    /// 匹配原工作和许可围栏并已提交。
    Applied(T),
    /// 围栏失效，无改动。
    Lost,
}

/// Worker 本地持有的依赖就绪判断，事务还须核 DB 及启用代次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingWorkerReadiness {
    /// schema、当前 keyring、opener 与 lookup 均就绪。
    Ready,
    /// 本地依赖有缺，不能续就绪心跳。
    Unavailable,
}

/// 解析工作、全局许可和心跳。不变量 2、5、6：每次领取、续租、退场和回收以 DB 时刻
/// 核验工作及许可并原子提交。`apply_lookup` 是唯一观察提交和 429 冷却记账入口。
pub trait LookupWorkStore: Send + Sync {
    /// 按单例锁后工作锁的顺序，分配全服务唯一的许可并先提交计次。
    /// 存储须在领取事务内读取并复核持久的外部访问暂停事实、准入与配置代次，
    /// 并与暂停写入串行化。暂停时返回 `Ok(None)`，不分配许可或扣减尝试预算。
    /// Worker 仍须在申请前和发送前复核暂停。
    fn claim(
        &self,
        request: LookupClaimRequest,
    ) -> ConsignmentFuture<'_, Option<ListingLookupClaim>>;

    /// 用原围栏续工作和许可的租期，以上一次授予的网络硬截止加 5 秒为上界，
    /// 出站截止保持不变。存储须在工作记录中持久化本次领取的原始网络硬截止，
    /// 供续租、回收与关停在 Worker 重启后沿用同一口径。
    fn renew(&self, fence: LookupFence) -> ConsignmentFuture<'_, LookupLeaseUpdate<Timestamp>>;

    /// 发网前复核持久围栏、代次及本次硬截止。暂停是 Worker 同步读取的最新暂停事实。
    fn confirm_dispatch<'a>(
        &'a self,
        claim: &'a ListingLookupClaim,
        paused: bool,
    ) -> ConsignmentFuture<'a, LookupDispatch>;

    /// 按远端状态安全让出执行权，不能倒扣计次或覆盖后来持有者。
    fn abandon(
        &self,
        fence: LookupFence,
        status: LookupAbandonment,
    ) -> ConsignmentFuture<'_, LookupLeaseUpdate<()>>;

    /// 有界回收失效租约，按原 fence CAS 重排、耗尽或到期；不得触发新的网络请求。
    fn reap_expired(&self) -> ConsignmentFuture<'_, u32>;

    /// 读取数据库时刻下的安全许可和冷却状态，仅用于调度判断。
    fn permit_status(&self) -> ConsignmentFuture<'_, LookupPermitStatus>;

    /// 只有本代次且装配就绪才可续至 DB 时刻后 30 秒；不可用时不延续旧心跳。
    fn record_worker_readiness(
        &self,
        service_epoch: u64,
        readiness: ListingWorkerReadiness,
    ) -> ConsignmentFuture<'_, ()>;
}
