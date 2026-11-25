//! 购买与 v2 改价共用的锁内事实快照；由存储车道在 Serializable 事务内按固定锁序读出后构造。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::consignment::deposit::PositionState;
use crate::consignment::{
    FeeBps, ListingMode, ListingPrice, ListingState, ListingVersion, SaleFundingFreeze, SeasonState,
};
use crate::{SettlementWorld, UserId};

/// 购买命令携带的预期；版本与价格都必须与锁内事实逐项相等。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchaseInput {
    pub listing_id: Uuid,
    pub expected_version: ListingVersion,
    pub expected_price: ListingPrice,
}

/// 锁内读出的挂单事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchaseListing {
    pub listing_id: Uuid,
    pub state: ListingState,
    pub version: ListingVersion,
    pub price: ListingPrice,
    pub world: SettlementWorld,
    pub mode: ListingMode,
    /// 登记时冻结的费率。
    pub fee_bps: FeeBps,
    /// 登记时冻结的计价算法版本。
    pub pricing_version: u16,
}

/// 锁内读出的代管 position 事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchasePosition {
    pub position_id: Uuid,
    pub state: PositionState,
    /// 当前处分代；没有处分代的 position 不能成交。
    pub generation: Option<u64>,
    /// 当前代是否已被某份合同消耗。
    pub generation_consumed: bool,
    /// 取得是否已由整封精确 Take 证明。
    pub take_proven: bool,
}

/// 交易当事人的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchaseParty {
    pub user_id: UserId,
    /// 账号仍有效（未停用）。
    pub active: bool,
    /// 具备交易资格（含黑名单、封禁等）。
    pub eligible: bool,
    pub is_authenticated_bot: bool,
}

/// 原认证 Bot main 资金账户的只读预检结论；Ready 自带 Sale 冻结值，判定与冻结同源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurchaseFunding {
    /// 预检可用；携带将冻结进 Sale 的账户事实。
    Ready(SaleFundingFreeze),
    /// 预检不可用（账户缺失、期初受阻、证据能力缺失等）。
    Unavailable,
}

/// 购买的锁内事实快照；领域不读时钟、不读库。
///
/// 约定（统筹方裁定，由 6A2 遵守）：
/// - `season` 必须由 6A2 用锁后的同一个 `now` 与冻结季窗算出；领域不读时钟，无法核对二者一致。
/// - `payment_deadline_days` 按 24 小时时长计算；D9 只对季窗规定本地日历日。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseFacts {
    /// 锁后数据库时刻；季界与期限均以等号为失效点。
    pub now: Timestamp,
    /// 寄售激活开关；关闭时不接受新承诺。
    pub service_enabled: bool,
    /// 挂单的服务世代等于当前世代。
    pub listing_epoch_current: bool,
    pub listing: PurchaseListing,
    /// 没有 position 即未代管。
    pub position: Option<PurchasePosition>,
    pub buyer: PurchaseParty,
    pub seller: PurchaseParty,
    /// 由调用方按 `now` 与冻结季窗算出的季态。
    pub season: SeasonState,
    /// 存在未解除的运营止售。
    pub active_hold: bool,
    /// 买家存在待执行的拒收处罚（新购阻断）。
    pub buyer_sanction_pending: bool,
    /// 当前代存在相反的未知外写。
    pub opposite_unknown_write: bool,
    /// 当前代存在未封闭交割；前代已闭合同可保留，不计入。
    pub unclosed_delivery: bool,
    /// 资金预检结论；调用方只读取一次，Sale 冻结值取自判定授予的同一份事实。
    pub funding: PurchaseFunding,
}
