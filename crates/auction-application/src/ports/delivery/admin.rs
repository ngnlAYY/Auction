use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{RequestHash, valid_page_window};

/// Admin 交付列表允许的最大单页大小。
pub const MAX_DELIVERY_ADMIN_PAGE_SIZE: i64 = 100;
/// Admin 交付列表允许的最大偏移，与管理拍卖、标的列表保持相同深分页上限。
pub const MAX_DELIVERY_ADMIN_OFFSET: i64 = 100_000;
/// Admin 交付投影允许显示的最大 COD 整数；与旧版 JSON 安全整数边界一致。
pub const MAX_DELIVERY_COD: i64 = 9_007_199_254_740_991;
/// 收件人筛选的最大字符数；与 Admin 输入框 maxlength 对齐。
pub const MAX_DELIVERY_ADMIN_RECIPIENT_LEN: usize = 256;
/// 冻结模板键的最大字符数；模板键由邮件流程生成，远短于该上限。
pub const MAX_DELIVERY_ADMIN_TEMPLATE_LEN: usize = 64;
/// 发送类别闭集；与 `public.delivery_send_category` 的返回集合一一对应。
pub const DELIVERY_ADMIN_CATEGORIES: [&str; 7] =
    ["outbid", "account", "intake", "settlement", "equipment", "operations", "manual"];
/// 世界闭集；投递载荷只允许这两个已发布世界。
pub const DELIVERY_ADMIN_WORLDS: [&str; 2] = ["main", "isekai"];
/// 退役原因闭集：在 sending 围栏前确定未写入外部系统。
pub const DELIVERY_ADMIN_RETIREMENT_REASONS: [&str; 5] = [
    "lot_watch_invalidated",
    "recipient_renamed",
    "auction_opened",
    "season_boundary_passed",
    "season_reminder_resolved",
];
/// 抑制原因闭集：在发送前被隔离、批次终止或载荷拒绝，从未开始外部写入。
pub const DELIVERY_ADMIN_SUPPRESSION_REASONS: [&str; 6] = [
    "mail_disposition_conflict",
    "invalid_payload",
    "task_panic",
    "dependency_mismatch",
    "retry_exhausted",
    "operations_alert_batch_terminal",
];

/// Delivery Admin 查询与命令使用的稳定业务错误。
/// 该类型只携带闭集分类，不跨应用边界暴露数据库驱动或持久化正文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAdminError {
    #[error("delivery Admin 命令参数无效")]
    Invalid,
    #[error("delivery 不存在")]
    NotFound,
    #[error("delivery 状态或 fencing version 已变化")]
    Stale,
    #[error("delivery Admin 命令与既有命令冲突")]
    CommandConflict,
    #[error("delivery Admin 持久化结果损坏")]
    StoredResultCorrupt,
    #[error("delivery Admin 依赖暂不可用")]
    Unavailable,
    /// Worker 已在 Sent 中目击同一结算义务的邮件；重试或判失败可能重复付款或误判，须走人工 Sent 证明。
    #[error("Sent 已出现该结算义务的邮件")]
    SentSighted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeliveryAdminListRequest {
    pub status: Option<String>,
    /// 收件人精确匹配（ASCII 大小写不敏感）；不含通配符。
    pub recipient: Option<String>,
    /// 参考编号：投递 ID、来源义务 ID 或玩家通知 ID 的规范 UUID 文本。
    pub reference: Option<Uuid>,
    /// 冻结的 MoogleMail 模板键。
    pub template: Option<String>,
    /// 发送类别闭集，见 [`DELIVERY_ADMIN_CATEGORIES`]。
    pub category: Option<String>,
    /// `main` / `isekai`。
    pub world: Option<String>,
    /// 创建时间下界（含）。
    pub created_from: Option<DateTime<Utc>>,
    /// 创建时间上界（含）。
    pub created_to: Option<DateTime<Utc>>,
    pub limit: i64,
    pub offset: i64,
}

impl DeliveryAdminListRequest {
    /// 校验有界查询，避免 Admin 端通过未限制的 offset 扫描整个 outbox。
    /// 所有筛选都是精确值；自由文本只允许已裁剪、无控制字符且有上限的单个值。
    pub fn validate(&self) -> bool {
        valid_page_window(
            self.limit,
            self.offset,
            MAX_DELIVERY_ADMIN_PAGE_SIZE,
            MAX_DELIVERY_ADMIN_OFFSET,
        ) && self.status.as_ref().is_none_or(|status| valid_delivery_status(status))
            && self.reference.is_none_or(|reference| !reference.is_nil())
            && self.recipient.as_deref().is_none_or(valid_recipient_filter)
            && self.template.as_deref().is_none_or(valid_template_filter)
            && self
                .category
                .as_deref()
                .is_none_or(|value| DELIVERY_ADMIN_CATEGORIES.contains(&value))
            && self.world.as_deref().is_none_or(|value| DELIVERY_ADMIN_WORLDS.contains(&value))
            && self.created_from.zip(self.created_to).is_none_or(|(from, to)| from <= to)
    }
}

/// 交付 Admin 允许的人工状态动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryAdminAction {
    Release,
    RetryFailed,
    ReconcileRetry,
    ReconcileSucceeded,
    ReconcileFailed,
}

impl DeliveryAdminAction {
    /// 返回持久化命令账本使用的稳定动作名。
    pub const fn command_kind(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::RetryFailed => "retry_failed",
            Self::ReconcileRetry | Self::ReconcileSucceeded | Self::ReconcileFailed => "reconcile",
        }
    }

    /// 按旧版已发布的字段顺序和 Go `encoding/json` 规则计算 Admin 命令摘要。
    /// `request_id` 有意不进入摘要。release/retry 只包含 delivery、fence 和 actor；reconcile
    /// 还包含动作及两个互斥结果字段，并把缺失值编码为空字符串。该字节合同必须保持稳定，
    /// 否则新旧实例滚动并存时会把同一命令误判为冲突。
    pub fn request_hash(
        self,
        delivery_id: Uuid,
        expected_fencing_version: i64,
        external_reference: Option<&str>,
        failure_code: Option<&str>,
        actor_id: &str,
    ) -> RequestHash {
        let mut payload = Vec::with_capacity(256);
        payload.extend_from_slice(b"{\"delivery_id\":");
        push_go_json_string(&mut payload, &delivery_id.to_string());
        payload.extend_from_slice(b",\"expected_fencing_version\":");
        payload.extend_from_slice(expected_fencing_version.to_string().as_bytes());

        if let Some(action) = self.reconcile_action_name() {
            payload.extend_from_slice(b",\"action\":");
            push_go_json_string(&mut payload, action);
            payload.extend_from_slice(b",\"external_reference\":");
            push_go_json_string(&mut payload, external_reference.unwrap_or_default());
            payload.extend_from_slice(b",\"failure_code\":");
            push_go_json_string(&mut payload, failure_code.unwrap_or_default());
        }

        payload.extend_from_slice(b",\"actor_id\":");
        push_go_json_string(&mut payload, actor_id);
        payload.push(b'}');
        RequestHash::from_canonical_bytes(&payload)
    }

    const fn reconcile_action_name(self) -> Option<&'static str> {
        match self {
            Self::Release | Self::RetryFailed => None,
            Self::ReconcileRetry => Some("retry"),
            Self::ReconcileSucceeded => Some("succeeded"),
            Self::ReconcileFailed => Some("failed"),
        }
    }
}

/// delivery outbox 的安全 Admin 投影。
/// 不包含 payload、命令摘要、lease token 或原始 result metadata；仅保留旧版契约允许的
/// `reason_code`、`take_required` 和 `returned_mail_id` 安全派生字段，避免把外部协议或敏感
/// 路由细节扩散到 UI。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAdminSnapshot {
    pub id: Uuid,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    /// 仅从版本化 `send` payload 提取的金额，不跨端口传递 payload 原文。
    pub cod: Option<i64>,
    pub status: String,
    /// 冻结的模板键；Return/Take 与无模板投递为 `None`。
    pub template: Option<String>,
    /// 发送类别；非 `send` 操作或旧载荷无法归类时为 `None`。
    pub category: Option<String>,
    /// 投递载荷的世界；旧载荷缺省按 `main` 归属。
    pub world: Option<String>,
    /// 围栏前退役原因；只在确定未写入外部系统时出现。
    pub retirement_reason: Option<String>,
    /// 围栏前抑制原因；只在确定未写入外部系统时出现。
    pub suppression_reason: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<DateTime<Utc>>,
    pub sending_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    pub reason_code: Option<String>,
    pub take_required: bool,
    pub returned_mail_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminPage {
    pub entries: Vec<DeliveryAdminSnapshot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdminCommand {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub action: DeliveryAdminAction,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAdminResult {
    pub command_id: Uuid,
    pub snapshot: DeliveryAdminSnapshot,
}

pub trait DeliveryAdminPort: Send + Sync {
    fn list(&self, request: DeliveryAdminListRequest) -> ApplicationFuture<'_, DeliveryAdminPage>;

    /// 读取单条投递的证据详情；不存在时返回 `DeliveryAdminError::NotFound`。
    fn detail(
        &self,
        delivery_id: Uuid,
    ) -> ApplicationFuture<'_, crate::ports::delivery::DeliveryAdminDetail>;

    fn execute(&self, command: DeliveryAdminCommand) -> ApplicationFuture<'_, DeliveryAdminResult>;

    /// 只读汇总某个玩家或某个参考编号关联的出价、成交、义务、投递与通知。
    fn player_support(
        &self,
        request: crate::ports::delivery::PlayerSupportRequest,
    ) -> ApplicationFuture<'_, crate::ports::delivery::PlayerSupportBundle>;
}

fn valid_delivery_status(value: &str) -> bool {
    matches!(
        value,
        "prepared"
            | "pending"
            | "claimed"
            | "sending"
            | "succeeded"
            | "failed"
            | "manual_reconcile"
    )
}

/// 收件人筛选只接受已裁剪、无控制字符且不超过上限的单个值；不做通配符展开。
fn valid_recipient_filter(value: &str) -> bool {
    !value.is_empty()
        && value == value.trim()
        && value.chars().count() <= MAX_DELIVERY_ADMIN_RECIPIENT_LEN
        && !value.chars().any(char::is_control)
}

/// 模板键来自邮件流程闭集，限制为小写字母、数字、下划线与连字符。
fn valid_template_filter(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DELIVERY_ADMIN_TEMPLATE_LEN
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

/// Go `encoding/json.Marshal` 默认启用 HTML-safe string；摘要必须逐字兼容该历史行为。
fn push_go_json_string(output: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    output.push(b'"');
    for character in value.chars() {
        match character {
            '"' => output.extend_from_slice(br#"\""#),
            '\\' => output.extend_from_slice(br#"\\"#),
            '\u{0008}' => output.extend_from_slice(br"\b"),
            '\u{000c}' => output.extend_from_slice(br"\f"),
            '\n' => output.extend_from_slice(br"\n"),
            '\r' => output.extend_from_slice(br"\r"),
            '\t' => output.extend_from_slice(br"\t"),
            '<' => output.extend_from_slice(br"\u003c"),
            '>' => output.extend_from_slice(br"\u003e"),
            '&' => output.extend_from_slice(br"\u0026"),
            '\u{2028}' => output.extend_from_slice(br"\u2028"),
            '\u{2029}' => output.extend_from_slice(br"\u2029"),
            character if character <= '\u{001f}' => {
                let value = character as u8;
                output.extend_from_slice(br"\u00");
                output.push(HEX[usize::from(value >> 4)]);
                output.push(HEX[usize::from(value & 0x0f)]);
            }
            character => {
                let mut encoded = [0_u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    output.push(b'"');
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::delivery::admin as parent;
    use crate::ports::delivery::{
        MAX_PLAYER_SUPPORT_USER_LEN, PlayerSupportRequest, PlayerSupportSelector,
    };
    use chrono::TimeZone;

    fn base() -> DeliveryAdminListRequest {
        DeliveryAdminListRequest { limit: 50, offset: 0, ..DeliveryAdminListRequest::default() }
    }

    #[test]
    fn filters_accept_closed_sets_and_bounded_text() {
        let request = DeliveryAdminListRequest {
            status: Some("failed".to_owned()),
            recipient: Some("Restia".to_owned()),
            reference: Some(Uuid::from_u128(7)),
            template: Some("settlement_seller_payout".to_owned()),
            category: Some("settlement".to_owned()),
            world: Some("isekai".to_owned()),
            created_from: Some(Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).single().expect("time")),
            created_to: Some(Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).single().expect("time")),
            ..base()
        };
        assert!(request.validate());
    }

    #[test]
    fn filters_reject_unknown_enums_and_untrimmed_text() {
        for request in [
            DeliveryAdminListRequest { category: Some("bogus".to_owned()), ..base() },
            DeliveryAdminListRequest { world: Some("moon".to_owned()), ..base() },
            DeliveryAdminListRequest { recipient: Some(" Restia".to_owned()), ..base() },
            DeliveryAdminListRequest { recipient: Some("Restia\n".to_owned()), ..base() },
            DeliveryAdminListRequest { template: Some("Bad Template".to_owned()), ..base() },
            DeliveryAdminListRequest { status: Some("unknown".to_owned()), ..base() },
            DeliveryAdminListRequest { reference: Some(Uuid::nil()), ..base() },
        ] {
            assert!(!request.validate(), "应当拒绝非法筛选: {request:?}");
        }
    }

    #[test]
    fn filters_reject_reversed_time_range_and_oversized_pages() {
        let later = Utc.with_ymd_and_hms(2026, 11, 2, 0, 0, 0).single().expect("time");
        let earlier = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).single().expect("time");
        assert!(
            !DeliveryAdminListRequest {
                created_from: Some(later),
                created_to: Some(earlier),
                ..base()
            }
            .validate()
        );
        assert!(!DeliveryAdminListRequest { limit: 0, ..base() }.validate());
        assert!(
            !DeliveryAdminListRequest { limit: MAX_DELIVERY_ADMIN_PAGE_SIZE + 1, ..base() }
                .validate()
        );
        assert!(
            !DeliveryAdminListRequest {
                recipient: Some("x".repeat(MAX_DELIVERY_ADMIN_RECIPIENT_LEN + 1)),
                ..base()
            }
            .validate()
        );
    }

    #[test]
    fn player_support_requires_exactly_one_bounded_selector() {
        assert!(
            PlayerSupportRequest { selector: PlayerSupportSelector::User("Restia".to_owned()) }
                .validate()
        );
        assert!(
            PlayerSupportRequest { selector: PlayerSupportSelector::Reference(Uuid::from_u128(1)) }
                .validate()
        );
        assert!(
            !PlayerSupportRequest { selector: PlayerSupportSelector::User(String::new()) }
                .validate()
        );
        assert!(
            !PlayerSupportRequest { selector: PlayerSupportSelector::User(" Restia".to_owned()) }
                .validate()
        );
        assert!(
            !PlayerSupportRequest {
                selector: PlayerSupportSelector::User("x".repeat(MAX_PLAYER_SUPPORT_USER_LEN + 1))
            }
            .validate()
        );
        // 形似 UUID 但不是规范文本时不能当成用户名去扫描。
        assert!(
            !PlayerSupportRequest {
                selector: PlayerSupportSelector::User(
                    "0f8fad5b-d9cb-469f-a165-70867728950".to_owned()
                )
            }
            .validate()
        );
    }
}
