//! 寄售模式、世界、状态、私有原因和卖家动作的线协议闭集。

use serde::Deserialize;
use serde::Serialize;

/// 挂单创建后保持不变的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingMode {
    /// 卖家自行交割。
    Direct,
    /// 平台代管模式，当前阶段尚未收件。
    Consigned,
}

/// 寄售装备所在世界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingWorld {
    /// 主世界。
    Main,
    /// 异世界。
    Isekai,
}

/// 公开目录可见的挂单状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicListingState {
    /// 非寄售广告已展示。
    Displayed,
    /// 寄售挂单等待装备。
    AwaitingDeposit,
}

/// 只有本人与 Admin 投影能读取的完整挂单状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingState {
    /// 解析中。
    Resolving,
    /// 已展示。
    Displayed,
    /// 等待寄来装备。
    AwaitingDeposit,
    /// 核验被拒。
    Rejected,
    /// 有效期已到。
    Expired,
    /// 挂单已关闭。
    Closed,
}

/// 终态的安全原因码，与领域的同名闭集分别定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingReason {
    /// 装备已进入拍卖。
    AuctionLotCreated,
    /// 卖家资格已失效。
    SellerIneligible,
    /// 服务已停用。
    ServiceDisabled,
    /// 卖家已取消。
    SellerCancelled,
    /// 运营已下架。
    AdminDelisted,
    /// 解析期限已到。
    LookupDeadline,
    /// 展示期限已到。
    DisplayExpired,
    /// 待寄期限已到。
    DepositExpired,
    /// 装备不可交易。
    NotTradeable,
    /// 来源缺少有效持有人。
    OwnerMissing,
    /// 来源的持有人与登记人不符。
    OwnerMismatch,
    /// 本站已证最后持有人与登记人不符。
    HistoryOwnerMismatch,
    /// 同件装备的历史证据冲突。
    HistoryAliasConflict,
    /// 交付或返还证据不足。
    HistoryUnproven,
    /// 已有挂单占用或未结拍卖责任。
    EquipmentBusy,
    /// 来源未找到装备或读取凭证无效。
    EquipmentNotFound,
    /// 来源拒绝请求。
    SourceRequestRejected,
    /// 来源 HTTP 响应不受支持。
    SourceHttpInvalid,
    /// 来源数据格式错误。
    SourceSchemaInvalid,
    /// 来源数据形态尚未确认。
    SourceShapeUnconfirmed,
    /// 来源装备标识不符。
    SourceIdentityMismatch,
    /// 来源响应超出预算。
    SourceTooLarge,
    /// 解析尝试次数耗尽。
    LookupExhausted,
    /// 本地装备读取凭证不可用。
    CredentialUnavailable,
}

/// 暂时读取失败的安全分类，可在解析进度和耗尽结果中显示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LookupReason {
    /// 暂时无法连接来源。
    SourceUnavailable,
    /// 读取超时。
    SourceTimeout,
    /// 来源限流。
    SourceRateLimited,
}

/// 由服务端判定并随本人视图返回的卖家操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingAction {
    /// 修改标价。
    Reprice,
    /// 取消挂单。
    Withdraw,
}
