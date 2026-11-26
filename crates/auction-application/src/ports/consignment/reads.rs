//! 寄售分权读模型、筛选、分页与登记可用性。

use std::fmt;
use std::str::FromStr;

use auction_domain::consignment::{
    FeeBps, ListingEquipmentIdentity, ListingMode, ListingPrice, ListingState, ListingVersion,
};
use auction_domain::{CommandId, LotId, UserId, valid_text};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::auction::MAX_PUBLIC_HISTORY_NAME_TERMS;
use crate::ports::consignment::commands::{ListingDelistReason, OwnedListingCommandReceipt};
use crate::ports::consignment::equipment::{ListingEquipmentDetails, RegistrationReplayCrypto};
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::consignment::projection::PrivateListingView;
use crate::ports::core::valid_page_window;

/// 寄售查询的世界闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingWorld {
    /// 常驻世界。
    Main,
    /// 赛季世界。
    Isekai,
}

impl ListingWorld {
    /// 数据库和查询契约使用的世界代码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }
}

impl FromStr for ListingWorld {
    type Err = ConsignmentError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "main" => Ok(Self::Main),
            "isekai" => Ok(Self::Isekai),
            _ => Err(ConsignmentError::InvalidQuery),
        }
    }
}

/// 当前允许登记的世界集。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ListingWorlds {
    /// 常驻世界可登记。
    pub main: bool,
    /// 赛季世界可登记。
    pub isekai: bool,
}

/// 读侧查询得到的持久依赖状态。应用层再与本地配置和 sealer 合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingRegistrationAvailability {
    /// 数据库、当前准入、比较钥和 Worker 新鲜心跳均可用。
    Ready(ListingWorlds),
    /// 至少一项持久依赖当前不可用。
    Unavailable,
}

/// 已检查的页码和页长，offset 不超过 10000。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingPageWindow {
    page: u32,
    page_size: u32,
}

impl ListingPageWindow {
    /// 应用默认页码和页长，并拒绝溢出或超出边界的输入。
    pub fn new(page: Option<u32>, page_size: Option<u32>) -> Result<Self, ConsignmentError> {
        let page = page.unwrap_or(1);
        let page_size = page_size.unwrap_or(20);
        let offset = (i64::from(page) - 1)
            .checked_mul(i64::from(page_size))
            .ok_or(ConsignmentError::InvalidQuery)?;
        if !valid_page_window(i64::from(page_size), offset, 200, 10_000) {
            return Err(ConsignmentError::InvalidQuery);
        }
        Ok(Self { page, page_size })
    }

    /// 对外的一基页码。
    pub const fn page(self) -> u32 {
        self.page
    }

    /// 本页最大记录数。
    pub const fn page_size(self) -> u32 {
        self.page_size
    }

    /// SQL 使用的零基偏移。
    pub const fn offset(self) -> u32 {
        // 构造时已核偏移范围。
        (self.page - 1) * self.page_size
    }
}

/// 一次目录请求最多可选的分类数，等于装备分类闭集（七类）的大小。
const MAX_LISTING_CATEGORIES: usize = 7;

/// 目录中规范的分类精确筛选，可含多个分类（命中任一即可）。
#[derive(Clone, PartialEq, Eq)]
pub struct ListingCategoryFilter(Vec<String>);

impl fmt::Debug for ListingCategoryFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 不变量 7：自由筛选文本可能被误填为装备 URL，禁止原样调试输出。
        formatter.write_str("[REDACTED]")
    }
}

impl ListingCategoryFilter {
    /// 解析逗号分隔的分类；单个值即旧写法。每项最多 128 UTF-8 字节，去重后保持首次出现
    /// 的顺序，空项、非法项与超过 `MAX_LISTING_CATEGORIES` 项（去重前计数）一律失败关闭。
    pub fn new(categories: String) -> Result<Self, ConsignmentError> {
        let mut items: Vec<String> = Vec::new();
        for (index, category) in categories.split(',').enumerate() {
            if index >= MAX_LISTING_CATEGORIES
                || !valid_text(category, 128)
                || category.contains(['\u{2028}', '\u{2029}'])
            {
                return Err(ConsignmentError::InvalidQuery);
            }
            if !items.iter().any(|item| item == category) {
                items.push(category.to_owned());
            }
        }
        Ok(Self(items))
    }

    /// 借用精确匹配的分类列表，至少一项。
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

/// 目录名称查询的最多片段数，与价格历史的名称词条上限相同。
const MAX_LISTING_NAME_TERMS: usize = MAX_PUBLIC_HISTORY_NAME_TERMS;

/// 目录名称查询拆出的片段；每段都须是名称或别名同一字段的字面子串。
#[derive(Clone, PartialEq, Eq)]
pub struct ListingNameQuery(Vec<String>);

impl fmt::Debug for ListingNameQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 不变量 7：自由筛选文本可能被误填为装备 URL，禁止原样调试输出。
        formatter.write_str("[REDACTED]")
    }
}

impl ListingNameQuery {
    /// 校验整串最多 80 个 Unicode 字符，再按空白或英文逗号拆成片段（与价格历史同口径）：
    /// 丢弃空片段，每段最多 128 UTF-8 字节；没有片段或超过 `MAX_LISTING_NAME_TERMS` 段时
    /// 失败关闭（不变量 6），不静默截断。
    pub fn new(query: String) -> Result<Self, ConsignmentError> {
        if !valid_text(&query, 80 * 4)
            || query.chars().count() > 80
            || query.contains(['\u{2028}', '\u{2029}'])
        {
            return Err(ConsignmentError::InvalidQuery);
        }
        let terms = query
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|term| !term.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if terms.is_empty()
            || terms.len() > MAX_LISTING_NAME_TERMS
            || terms.iter().any(|term| term.len() > 128)
        {
            return Err(ConsignmentError::InvalidQuery);
        }
        Ok(Self(terms))
    }

    /// 借用片段，至少一项。数据库大小写折叠，`%`、`_`、`\` 不作通配符或转义。
    pub fn terms(&self) -> &[String] {
        &self.0
    }
}

/// 公开目录的查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingQuery {
    /// 页窗口。
    pub window: ListingPageWindow,
    /// 可选的世界筛选。
    pub world: Option<ListingWorld>,
    /// 可选的模式筛选。
    pub mode: Option<ListingMode>,
    /// 可选的精确装备分类（命中任一）。
    pub category: Option<ListingCategoryFilter>,
    /// 可选的名称片段，须全部落在装备名或全部落在别名中。
    pub name: Option<ListingNameQuery>,
}

/// 本人与 Admin 共用的私有查询条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivateListingQuery {
    /// 页窗口。
    pub window: ListingPageWindow,
    /// 可选的世界筛选。
    pub world: Option<ListingWorld>,
    /// 可选的模式筛选。
    pub mode: Option<ListingMode>,
    /// 按有效态筛选。
    pub state: Option<ListingState>,
}

/// Admin 额外可以筛选精确的卖家 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminListingQuery {
    /// 世界、模式、有效状态及分页。
    pub listings: PrivateListingQuery,
    /// 若有值，只读取该卖家的记录。
    pub seller_user_id: Option<UserId>,
}

/// 从同一数据库快照取得的一页记录及精确总数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingReadPage<T> {
    /// 按 `updated_at DESC,id DESC` 排序的本页，合法空页仍有总数。
    pub items: Vec<T>,
    /// 同筛选集下跨页总数。
    pub total: u64,
    /// 返回窗口。
    pub window: ListingPageWindow,
}

/// 公开记录只存在这两种模式、状态组合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicListingState {
    /// 已公开的非寄售广告。
    Displayed,
    /// 已公开的待寄摘要，前三期无代管或购买能力。
    AwaitingDeposit,
}

/// 公开目录所需的白名单摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSummary {
    /// 挂单身份。
    pub listing_id: Uuid,
    /// 已提交版本。
    pub version: ListingVersion,
    /// 公开态及相应模式。
    pub state: PublicListingState,
    /// 规范世界和 EID。
    pub identity: ListingEquipmentIdentity,
    /// 展示名称。
    pub name: String,
    /// 装备分类。
    pub category: String,
    /// 安全等级。
    pub level: crate::ports::consignment::equipment::ListingEquipmentLevel,
    /// 标价。
    pub price: ListingPrice,
    /// 当前本站卖家游戏名；匿名挂单为登记人自填的名称。
    pub seller_name: String,
    /// 卖家经站内账户登记；匿名非寄售挂单为否，展示为“卖家自填、未验证”。
    pub seller_verified: bool,
    /// 最后一次持久变化。
    pub updated_at: Timestamp,
    /// 本态截止。
    pub expires_at: Timestamp,
}

/// 独立 public 详情，不夹带会话或所有权数据。
#[derive(Debug, Clone, PartialEq)]
pub struct PublicListingDetail {
    /// 和目录同判定的公开摘要。
    pub summary: PublicListingSummary,
    /// 装备白名单详情。
    pub equipment: ListingEquipmentDetails,
}

/// 权限化私有读投影的共同事实，无动作授权。
#[derive(Debug, Clone, PartialEq)]
pub struct PrivateListing {
    /// 挂单身份。
    pub listing_id: Uuid,
    /// 持久版本，有效读不增加版本。
    pub version: ListingVersion,
    /// 规范世界和 EID。
    pub identity: ListingEquipmentIdentity,
    /// 当前标价。
    pub price: ListingPrice,
    /// 冻结定价算法版本，前三期为 1。
    pub pricing_version: u16,
    /// 登记时冻结的费率，应用层用领域报价函数计算预计金额。
    pub fee_bps: FeeBps,
    /// 创建时刻。
    pub created_at: Timestamp,
    /// 最近持久变化。
    pub updated_at: Timestamp,
    /// 当前态冻结截止。
    pub expires_at: Timestamp,
    /// 同快照的登记模式、持久状态及有效视图。
    pub view: PrivateListingView,
}

/// 服务端允许卖家执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SellerListingAction {
    /// 修改标价。
    Reprice,
    /// 取消挂单。
    Withdraw,
}

/// 会话所有者的私有列表行及详情。
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedListingRecord {
    /// 版本、持久状态和有效状态。
    listing: PrivateListing,
    /// 锁内写入仍须重新检查的卖家可用动作。
    allowed_actions: Vec<SellerListingAction>,
}

impl OwnedListingRecord {
    /// 不变量 6：有效终态不得提供卖家动作。
    pub fn new(
        listing: PrivateListing,
        allowed_actions: Vec<SellerListingAction>,
    ) -> Result<Self, ConsignmentError> {
        if !allowed_actions.is_empty() && !listing.view.effective_state().0.is_active() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { listing, allowed_actions })
    }

    /// 借用挂单事实。
    pub const fn listing(&self) -> &PrivateListing {
        &self.listing
    }

    /// 卖家可用动作，写入事务仍须重新检查。
    pub fn allowed_actions(&self) -> &[SellerListingAction] {
        &self.allowed_actions
    }
}

/// 管理员查询行，无冒充卖家的动作接口。
#[derive(Debug, Clone, PartialEq)]
pub struct AdminListingRecord {
    /// 管理员可读的挂单共同事实。
    listing: PrivateListing,
    /// 真正的本地卖家；匿名非寄售挂单没有本站账户。
    pub seller_user_id: Option<UserId>,
    /// 当前本站卖家名；匿名挂单为登记人自填的名称。
    pub seller_name: String,
    /// 仅运营会话可查看的下架说明。
    pub admin_delist_reason: Option<ListingDelistReason>,
    /// 当前允许下架。
    can_delist: bool,
}

impl AdminListingRecord {
    /// 不变量 6：有效终态不得提供运营下架动作。
    pub fn new(
        listing: PrivateListing,
        seller_user_id: Option<UserId>,
        seller_name: String,
        admin_delist_reason: Option<ListingDelistReason>,
        can_delist: bool,
    ) -> Result<Self, ConsignmentError> {
        if can_delist && !listing.view.effective_state().0.is_active() {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(Self { listing, seller_user_id, seller_name, admin_delist_reason, can_delist })
    }

    /// 借用 Admin 可读的挂单事实。
    pub const fn listing(&self) -> &PrivateListing {
        &self.listing
    }

    /// 当前是否允许下架。
    pub const fn can_delist(&self) -> bool {
        self.can_delist
    }
}

/// Admin 详情的一条实际拍卖碰撞。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingAuctionConflict {
    /// 已提交的 lot。
    pub lot_id: LotId,
    /// 检知时刻。
    pub detected_at: Timestamp,
}

/// Admin 详情，包括最近 50 条碰撞及截断信息。
#[derive(Debug, Clone, PartialEq)]
pub struct AdminListingDetail {
    /// 管理员读投影。
    pub record: AdminListingRecord,
    /// 按 `detected_at DESC,lot_id DESC` 排序。
    pub conflicts: Vec<ListingAuctionConflict>,
    /// 尚有更早的关联。
    pub conflicts_truncated: bool,
}

/// 匿名登记的原决定及挂单当前事实。
#[derive(Debug, Clone, PartialEq)]
pub struct AnonymousRegistrationReceipt {
    /// 账本中的原登记决定。
    pub command: OwnedListingCommandReceipt,
    /// 受理后挂单的当前状态与原因；被拒绝的登记没有挂单。
    pub listing: Option<PrivateListing>,
}

/// 匿名公开读，缺少依赖时返回错误而非空目录。
pub trait PublicListingRead: Send + Sync {
    /// 公开行和总数均使用 §4.2 的完整可见性谓词。
    fn list_public(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicListingSummary>>;

    /// 不存在和不可见均返回 `None`。
    fn public_detail(&self, listing_id: Uuid)
    -> ConsignmentFuture<'_, Option<PublicListingDetail>>;

    /// 读取持久准入、登记世界、新鲜心跳与比较钥能力。首次尚无逻辑钥时，依赖必须具备
    /// 初始化能力才能就绪。此读不向上游探测，也不写入初始化行。
    fn registration_availability<'a>(
        &'a self,
        replay_crypto: &'a dyn RegistrationReplayCrypto,
    ) -> ConsignmentFuture<'a, ListingRegistrationAvailability>;

    /// 只读匿名 actor 的登记命令；不可猜测的 UUIDv4 命令号即读取凭据。
    /// 未读取到行不证明未知提交已经失败。
    fn anonymous_command(
        &self,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<AnonymousRegistrationReceipt>>;
}

/// 仅从当前已守卫的会话取得卖家 ID，再读取本人历史。
pub trait OwnedListingRead: Send + Sync {
    /// 按卖家和有效态筛选，同快照取总数。
    fn list_owned(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedListingRecord>>;

    /// 非本人和不存在均返回 `None`。
    fn owned_detail(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedListingRecord>>;

    /// 返回原命令决定，仅允许原玩家 actor。未读取到行不证明未知提交已经失败。
    fn owned_command(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedListingCommandReceipt>>;
}

/// Admin 授权后的只读端口，关闭时依旧可读历史。
pub trait AdminListingRead: Send + Sync {
    /// 根据有效态及可选卖家筛选，同快照取总数。
    fn list_admin(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>>;

    /// 查询私有详情和安全碰撞关联。
    fn admin_detail(&self, listing_id: Uuid) -> ConsignmentFuture<'_, Option<AdminListingDetail>>;
}
