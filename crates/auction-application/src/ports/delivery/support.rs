//! 玩家支持只读汇总端口。
//!
//! 按用户名（或用户 ID）或一个参考编号汇总出价、成交、义务、投递与通知，供 Admin 处理
//! 玩家咨询与核对冒充 Bot 的邮件。所有分区都有固定行数上限，且只返回安全投影：不含投递
//! payload、正文、Cookie、登录 key、会话凭据或代理上限。

use auction_domain::valid_text;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::delivery::DeliveryAdminSnapshot;

/// 每个分区最多返回的行数；超过时置 `truncated`，不静默分页。
pub const MAX_PLAYER_SUPPORT_ROWS: i64 = 50;
/// 用户名筛选的最大 UTF-8 字节数，与用户名写入、HTTP 查询使用同一边界。
pub const MAX_PLAYER_SUPPORT_USER_LEN: usize = 256;

/// 只读汇总的检索入口；两者互斥，必须严格提供一个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerSupportSelector {
    /// 已裁剪的用户名或规范用户 UUID 文本。
    User(String),
    /// 投递、义务、通知、lot 或场次的规范 UUID 参考编号。
    Reference(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportRequest {
    pub selector: PlayerSupportSelector,
    /// 是否让结算退件 Take 投递（来源 `settlement_return_action`）出现在汇总里。缺省不出现：已发布
    /// 的 Admin 页面按闭集解码来源，读到不认识的来源整份汇总失败，所以只有显式请求扩展投影的页面
    /// 才看到它们。缺省时这类投递不进入投递分区（行数上限与截断按同一口径计算）、不按参考编号补入，
    /// 也不作为参考编号的种类或推导关联用户的依据；被省略的投递在库中不变。
    pub include_settlement_return_actions: bool,
}

impl PlayerSupportRequest {
    /// 校验单一检索入口；UUID 形用户输入与参考编号都必须为规范非零 UUID。
    pub fn validate(&self) -> bool {
        match &self.selector {
            PlayerSupportSelector::User(value) => valid_user_selector(value),
            PlayerSupportSelector::Reference(reference) => !reference.is_nil(),
        }
    }
}

fn valid_user_selector(value: &str) -> bool {
    if !valid_text(value, MAX_PLAYER_SUPPORT_USER_LEN) {
        return false;
    }
    // 存储会把可解析的 UUID 当作用户 ID；非规范拼写与残缺 ID 不能回落到用户名查询。
    if let Ok(parsed) = Uuid::parse_str(value) {
        return !parsed.is_nil() && parsed.hyphenated().to_string() == value;
    }
    let mut groups = value.split('-');
    let has_uuid_prefix = [8, 4, 4, 4]
        .into_iter()
        .all(|width| groups.next().is_some_and(|group| group.len() == width));
    let tail = groups.next().unwrap_or_default();
    let uuid_shaped = has_uuid_prefix
        && groups.next().is_none()
        && (tail.len() == 12
            || tail.len() < 12
                && value.bytes().all(|byte| byte == b'-' || byte.is_ascii_hexdigit()));
    !uuid_shaped
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportUser {
    pub id: Uuid,
    pub username: String,
    pub status: String,
    /// 游戏内身份；MoogleMail 收件人与结算义务按它关联。
    pub game_player_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportBid {
    pub bid_id: Uuid,
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub world_id: String,
    pub amount: i64,
    pub mechanism: String,
    pub lot_status: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportDeal {
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub world_id: String,
    pub equipment_name: String,
    pub final_price: i64,
    pub effective_end_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportObligation {
    pub obligation_id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub world_id: String,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    pub fulfillment_state: Option<String>,
    pub fulfillment_error_code: Option<String>,
    pub delivery_id: Option<Uuid>,
}

/// 通知分区统一投影；`channel` 与 `state` 是闭集字符串，`state_reason` 只含已校验的失败或退役原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportNotification {
    pub notification_id: Uuid,
    /// `mm`（MoogleMail 投递）、`pm`（论坛私信）或 `pending`（待定提醒）。
    pub channel: String,
    /// 冻结模板键或通知身份名。
    pub kind: Option<String>,
    pub lot_id: Option<Uuid>,
    pub auction_id: Option<Uuid>,
    pub state: String,
    pub state_reason: Option<String>,
    pub delivery_id: Option<Uuid>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// 命中的参考编号实体类型；让操作员确认这个编号究竟指向哪类记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportReference {
    pub kind: String,
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerSupportBundle {
    pub user: Option<PlayerSupportUser>,
    pub bids: Vec<PlayerSupportBid>,
    pub deals: Vec<PlayerSupportDeal>,
    pub obligations: Vec<PlayerSupportObligation>,
    pub deliveries: Vec<DeliveryAdminSnapshot>,
    pub notifications: Vec<PlayerSupportNotification>,
    pub references: Vec<PlayerSupportReference>,
    /// 任一分区达到行数上限时置真；调用方不得把它当成完整历史。
    pub truncated: bool,
}
