//! 第 6 期订单读端口与公开 v2 目录读端口：本人与 Admin 的订单列表 / 详情 / 证据分页，以及可购挂单。
//!
//! 记录类型只含订单阶段枚举、金额与 ID；不含预约码、装备 key、对方账号或原信正文（不变量 7）。
//! 订单态与阶段只由领域 `project_order_stages` 投影，端口层不再写第二份规则；金额按角色省略在
//! [`OwnedOrder::new`] 构造时完成，调用方拿不到“买方的手续费”这种不该出现的组合。

use auction_domain::UserId;
use auction_domain::consignment::{
    ListingEquipmentIdentity, ListingPrice, ListingVersion, OrderKind, OrderStages, OrderState,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::equipment::ListingEquipmentDetails;
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::consignment::reads::{
    ListingPageWindow, ListingReadPage, ListingWorld, PublicListingQuery,
};

/// 购买能力的验证器签收清单（R5-C10）：按世界给出“原件 / 载体 / 现金验证器已签收”。
///
/// 本期全为 false：样本与签收是单独提交，翻转它之前 `purchasable` 与 `purchase_ready` 恒假，
/// 生产环境只能隔离与对账。不得由配置或环境变量覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurchaseValidatorsSigned {
    /// main 世界。
    pub main: bool,
    /// isekai 世界。
    pub isekai: bool,
}

/// 当前的签收清单。
pub const CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED: PurchaseValidatorsSigned =
    PurchaseValidatorsSigned { main: false, isekai: false };

impl PurchaseValidatorsSigned {
    /// 指定世界是否已签收。
    pub const fn signed(self, world: ListingWorld) -> bool {
        match world {
            ListingWorld::Main => self.main,
            ListingWorld::Isekai => self.isekai,
        }
    }
}

/// 读取者在订单里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRole {
    /// 买家。
    Buyer,
    /// 卖家。
    Seller,
}

/// 本人订单列表的筛选；状态取投影后的订单态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderQuery {
    /// 页窗口。
    pub window: ListingPageWindow,
    /// 只看某一角色。
    pub role: Option<OrderRole>,
    /// 只看某一世界。
    pub world: Option<ListingWorld>,
    /// 只看某一订单态。
    pub state: Option<OrderState>,
}

/// Admin 订单列表的筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminOrderQuery {
    /// 页窗口。
    pub window: ListingPageWindow,
    /// 只看某一世界。
    pub world: Option<ListingWorld>,
    /// 只看某一订单态。
    pub state: Option<OrderState>,
}

/// 订单的角色无关事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderCore {
    /// 合同 ID。
    pub contract_id: Uuid,
    /// 成交或撤回。
    pub kind: OrderKind,
    /// 来源挂单。
    pub listing_id: Uuid,
    /// 代管装备 ID。
    pub equipment_id: Uuid,
    /// 世界。
    pub world: ListingWorld,
    /// 投影后的订单态与阶段。
    pub stages: OrderStages,
    /// 订单版本：订单阶段事件流的头部序号，至少为 1。
    pub version: u64,
    /// 成交价；撤回订单没有。
    pub price: Option<ListingPrice>,
    /// 付款期限；撤回订单没有。
    pub deadline_at: Option<Timestamp>,
    /// 冻结手续费；撤回订单没有。
    pub fee: Option<i64>,
    /// 冻结到手金额；撤回订单没有。
    pub seller_amount: Option<i64>,
    /// 最近一次订单阶段变化时刻。
    pub updated_at: Timestamp,
}

/// 本人视角的订单：金额已按角色省略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedOrder {
    core: OrderCore,
    role: OrderRole,
}

impl OwnedOrder {
    /// 不变量 7、10：只有卖家的成交订单保留手续费与到手金额，买家与撤回订单一律清空。
    pub fn new(mut core: OrderCore, role: OrderRole) -> Self {
        if role == OrderRole::Buyer || core.kind == OrderKind::Withdrawal {
            core.fee = None;
            core.seller_amount = None;
        }
        Self { core, role }
    }

    /// 角色无关事实（金额已按角色省略）。
    pub const fn core(&self) -> &OrderCore {
        &self.core
    }

    /// 读取者角色。
    pub const fn role(&self) -> OrderRole {
        self.role
    }
}

/// Admin 视角的订单：买卖双方与完整金额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminOrder {
    /// 角色无关事实，金额不省略。
    pub core: OrderCore,
    /// 买家本地账号；撤回订单没有。
    pub buyer_user_id: Option<UserId>,
    /// 买家冻结身份；撤回订单没有。
    pub buyer_name: Option<String>,
    /// 卖家本地账号。
    pub seller_user_id: UserId,
    /// 卖家冻结身份。
    pub seller_name: String,
}

/// 证据条目；枚举值用线协议码（`&'static str`），由 HTTP 层按 DTO 闭集解码，未知值失败关闭。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderEvidence {
    /// 派发轮。
    Round {
        /// 轮 ID。
        round_id: Uuid,
        /// 轮号。
        round_number: u32,
        /// 所属腿。
        leg: &'static str,
        /// 该轮的交割阶段码。
        delivery_stage: &'static str,
        /// 建轮时刻。
        created_at: Timestamp,
    },
    /// 派发尝试。
    Attempt {
        /// 尝试 ID。
        attempt_id: Uuid,
        /// 所属轮。
        round_id: Uuid,
        /// 起写 fence。
        fence: u64,
        /// 是否已起写。
        write_started: bool,
        /// 交割阶段码。
        delivery_stage: &'static str,
    },
    /// 退件接管。
    ReturnTake {
        /// 接管 ID。
        intake_id: Uuid,
        /// Take 工作 ID。
        work_id: Uuid,
        /// 原轮 ID。
        original_round_id: Uuid,
        /// Take 起写 fence；未起写时为空。
        fence: Option<u64>,
        /// 进度码。
        state: &'static str,
    },
    /// 远端观察。
    Observation {
        /// 观察 ID。
        observation_id: Uuid,
        /// 类别码。
        category: &'static str,
        /// 结论码。
        conclusion: &'static str,
        /// 观察摘要，小写十六进制。
        digest: String,
        /// 观察时刻。
        observed_at: Timestamp,
    },
    /// 现金用途。
    CashUse {
        /// 用途码。
        purpose: &'static str,
        /// 该用途的阶段码。
        state: &'static str,
        /// 金额。
        amount: i64,
    },
    /// 阻断。
    Blocker {
        /// 阻断类别码。
        blocker: &'static str,
        /// 起始时刻。
        since: Timestamp,
        /// 是否已解除。
        resolved: bool,
    },
    /// Sale 封闭事实（恢复段，第 7 期）。
    Closure {
        /// 封闭种类码。
        closure_kind: &'static str,
        /// 封闭时刻。
        closed_at: Timestamp,
    },
    /// Admin 处置记录（恢复段，第 7 期）。
    Resolution {
        /// 处置 ID。
        resolution_id: Uuid,
        /// 动作码。
        action: &'static str,
        /// 处置依据的证据 ID。
        evidence_id: Uuid,
        /// 记录时刻。
        resolved_at: Timestamp,
    },
    /// Admin 排入的复核工作（恢复段，第 7 期）。
    Recheck {
        /// 复核工作 ID。
        work_id: Uuid,
        /// 排队时刻。
        requested_at: Timestamp,
    },
}

/// 本人订单读端口。非本人与不存在同样返回空，调用方不得区分。
pub trait OwnedOrderRead: Send + Sync {
    /// 本人作为买家或卖家的订单，按建单时间倒序。
    fn list_owned_orders(
        &self,
        user_id: UserId,
        query: OrderQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedOrder>>;

    /// 本人订单详情；非本人或不存在为 `None`。
    fn owned_order(
        &self,
        user_id: UserId,
        contract_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedOrder>>;
}

/// Admin 订单读端口。
pub trait AdminOrderRead: Send + Sync {
    /// 全部订单，按建单时间倒序。
    fn list_admin_orders(
        &self,
        query: AdminOrderQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminOrder>>;

    /// 订单详情；不存在为 `None`。
    fn admin_order(&self, contract_id: Uuid) -> ConsignmentFuture<'_, Option<AdminOrder>>;

    /// 订单证据分页；订单不存在为 `None`。
    fn order_evidence(
        &self,
        contract_id: Uuid,
        window: ListingPageWindow,
    ) -> ConsignmentFuture<'_, Option<ListingReadPage<OrderEvidence>>>;
}

/// 公开 v2 目录里的一条挂单：展示字段、处分代与购买能力。
#[derive(Debug, Clone, PartialEq)]
pub struct PublicCatalogEntryV2 {
    /// 挂单 ID。
    pub listing_id: Uuid,
    /// 已提交版本。
    pub version: ListingVersion,
    /// 持久状态：displayed、awaiting_deposit 或 available。
    pub state: auction_domain::consignment::ListingState,
    /// 登记模式。
    pub mode: auction_domain::consignment::ListingMode,
    /// 规范世界与 EID。
    pub identity: ListingEquipmentIdentity,
    /// 展示名称。
    pub name: String,
    /// 装备分类。
    pub category: String,
    /// 安全等级。
    pub level: crate::ports::consignment::equipment::ListingEquipmentLevel,
    /// 标价。
    pub price: ListingPrice,
    /// 卖家名。
    pub seller_name: String,
    /// 卖家经站内账户登记；匿名非寄售挂单为否，卖家名是未经验证的自填游戏名。
    pub seller_verified: bool,
    /// 最近持久变化。
    pub updated_at: Timestamp,
    /// 本态截止。
    pub expires_at: Timestamp,
    /// 处分代；`available` 必有，其余为空。
    pub generation: Option<u64>,
    /// 此刻可购买：`available` 且该世界的购买能力门全部成立。
    pub purchasable: bool,
}

/// 公开 v2 详情：目录条目与同一行的白名单装备快照。
#[derive(Debug, Clone, PartialEq)]
pub struct PublicCatalogDetailV2 {
    /// 与目录同判定的条目。
    pub entry: PublicCatalogEntryV2,
    /// 装备详情。
    pub equipment: ListingEquipmentDetails,
}

/// 公开 v2 目录读端口；`available` 的挂单可见，购买能力随世界能力门计算。
pub trait PublicCatalogReadV2: Send + Sync {
    /// 公开目录页。
    fn list_public_v2(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicCatalogEntryV2>>;

    /// 公开详情；不可见为 `None`。
    fn public_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PublicCatalogDetailV2>>;
}
