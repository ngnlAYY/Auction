//! Admin 投递列表与详情的响应投影。
//!
//! 投递来源与处置种类是浏览器按闭集解码的值。部署窗口内升级前打开的页面仍会读取新服务端，缺省
//! 响应多出一个它不认识的值，整页列表或详情就解码失败，直到刷新。因此缺省投影只输出已发布页面
//! 认识的值：列表不列出结算退件 Take（来源 `settlement_return_action`）与被恢复终结的原结算投递
//! （`failed/recovery_superseded`，没有发送时间），详情拒绝读取这两类投递，处置只保留已发布的
//! 种类闭集。本版本的页面以 `include=settlement_returns` 显式请求扩展投影（见
//! `crates/auction-contracts/README.md` 的变更规则）。与其他路由一致，`include` 每次只接受一个
//! 取值，所以列表与详情共用这一个取值，覆盖本版本新增的来源与处置种类。

use auction_contracts::AdminDeliveryDetailResponse;
use auction_domain::DeliveryDispositionKind;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::errors::{api_error, invalid_query};

/// 新页面请求扩展投影时的 `include` 取值；列表与详情共用。
const INCLUDE_SETTLEMENT_RETURNS: &str = "settlement_returns";

/// 结算退件 Take 投递的来源（迁移 `000114` 新增）；缺省投影不输出。
const SETTLEMENT_RETURN_ACTION_ORIGIN: &str = "settlement_return_action";

/// 缺省详情可以出现的处置种类：Admin 命令账本的三种动作、人工 Sent 证明命令与统一处置表在
/// `000113` 之前的八种，即已发布页面解码时认可的闭集。用允许清单而不是排除清单，之后新增的种类
/// 默认同样不出现。
const PUBLISHED_DISPOSITION_KINDS: [&str; 12] = [
    "release",
    "retry_failed",
    "reconcile",
    "sent_operator_confirmed",
    "operator_acknowledged",
    "auto_sent_proof",
    "operator_sent_confirmation",
    "admin_failed",
    "equipment_loss",
    "cod_returned",
    "legacy_self",
    "preflight_retired",
];

/// 恢复接管退件后由恢复 owner 终结的原结算投递的失败码：从未进入发送时间链，已发布页面的
/// 解码不认，缺省投影不输出（与存储的列表排除同一口径）。
const RECOVERY_SUPERSEDED_CODE: &str = "recovery_superseded";

fn recovery_superseded(detail: &AdminDeliveryDetailResponse) -> bool {
    detail.delivery.status == "failed"
        && detail.delivery.failure_code.as_deref() == Some(RECOVERY_SUPERSEDED_CODE)
}

/// 投递列表与详情的响应投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryProjection {
    /// 已发布的缺省投影：不含结算退件 Take 投递，处置只含已发布的种类闭集。
    Published,
    /// `include=settlement_returns`：列出结算退件 Take 投递，详情追加装备收回处置
    /// `recovery_equipment_recovered`（迁移 `000113` 新增）。之后新增的值不随它输出。
    SettlementReturns,
    /// `include=disposition_actions`：详情额外附带当前 fence 上的处置准入。
    DispositionActions,
}

impl DeliveryProjection {
    /// 解析已取出的 `include` 取值：缺省是已发布投影，只有精确的 `settlement_returns` 是扩展投影；
    /// 其他取值（含空值、大小写不同、逗号列表与其他路由的取值）为 400 `invalid_query`（附加字段
    /// 无效），不静默退回缺省投影。重复出现的 `include` 已由 `parse_query` 拒绝。
    pub(crate) fn parse(include: Option<&str>) -> Result<Self, Box<Response>> {
        match include {
            None => Ok(Self::Published),
            Some(INCLUDE_SETTLEMENT_RETURNS) => Ok(Self::SettlementReturns),
            Some(_) => Err(Box::new(invalid_query("附加字段无效"))),
        }
    }

    /// 列表是否列出结算退件 Take 投递；由存储在同一条件下计数与分页，不在页内事后过滤。
    pub(crate) const fn includes_settlement_return_actions(self) -> bool {
        matches!(self, Self::SettlementReturns | Self::DispositionActions)
    }

    /// 按投影输出详情。缺省投影下结算退件 Take 与被恢复终结的原结算投递返回 404
    /// `delivery_projection_excluded`，与列表不列出它们同一口径，不把来源或状态改写成旧页面认识的
    /// 值；处置只保留投影允许的种类，被省略的事实仍只追加在库中，新页面显式请求即可看到。
    pub(crate) fn project_detail(
        self,
        mut detail: AdminDeliveryDetailResponse,
    ) -> Result<AdminDeliveryDetailResponse, Box<Response>> {
        if !self.includes_settlement_return_actions()
            && (detail.delivery.origin_kind == SETTLEMENT_RETURN_ACTION_ORIGIN
                || recovery_superseded(&detail))
        {
            return Err(Box::new(
                api_error(
                    StatusCode::NOT_FOUND,
                    "delivery_projection_excluded",
                    "该投递不在缺省投影内，须带 include=settlement_returns 读取",
                )
                .into_response(),
            ));
        }
        detail.dispositions.retain(|item| self.disposition_visible(&item.kind));
        Ok(detail)
    }

    fn disposition_visible(self, kind: &str) -> bool {
        PUBLISHED_DISPOSITION_KINDS.contains(&kind)
            || (self.includes_settlement_return_actions()
                && kind == DeliveryDispositionKind::RecoveryEquipmentRecovered.as_str())
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::{
        AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
        AdminDeliveryDispositionResponse, AdminDeliveryResponse,
    };
    use auction_domain::DeliveryDispositionKind;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::Response;
    use serde_json::Value;

    use super::DeliveryProjection;

    /// 升级前已发布的 Admin 页面（v0.1.5）解码投递时认可的来源闭集（`valid_origin_kind`）；这里
    /// 独立抄录，不引用实现常量，代表仍在浏览器里运行的旧 decoder。
    const RELEASED_DECODER_ORIGINS: [&str; 9] = [
        "settlement",
        "settlement_recovery_action",
        "user_key",
        "manual",
        "mail_intake",
        "operations_alert",
        "lot_watch",
        "seller_summary",
        "season_reminder",
    ];

    /// 同一旧页面解码投递详情时认可的处置种类闭集（`valid_disposition`），同样独立抄录。
    const RELEASED_DECODER_DISPOSITIONS: [&str; 12] = [
        "release",
        "retry_failed",
        "reconcile",
        "sent_operator_confirmed",
        "operator_acknowledged",
        "auto_sent_proof",
        "operator_sent_confirmation",
        "admin_failed",
        "equipment_loss",
        "cod_returned",
        "legacy_self",
        "preflight_retired",
    ];

    /// 数据库 `deliveries_origin_kind_check` 当前允许的全部来源（迁移 `000114`）。
    const STORED_ORIGINS: [&str; 10] = [
        "settlement",
        "settlement_recovery_action",
        "settlement_return_action",
        "user_key",
        "manual",
        "mail_intake",
        "operations_alert",
        "lot_watch",
        "seller_summary",
        "season_reminder",
    ];

    fn disposition(kind: &str) -> AdminDeliveryDispositionResponse {
        AdminDeliveryDispositionResponse {
            command_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            kind: kind.to_owned(),
            occurred_at: "2026-11-16T00:00:00Z".to_owned(),
            status: None,
            fencing_version: Some(2),
            failure_code: None,
            external_reference: None,
            actor: Some("admin".to_owned()),
            reason: None,
        }
    }

    /// 带全部处置种类（统一处置表的全部种类加命令账本与人工 Sent 证明的种类）的详情。
    fn detail(origin_kind: &str) -> AdminDeliveryDetailResponse {
        AdminDeliveryDetailResponse {
            delivery: AdminDeliveryResponse {
                id: "00000000-0000-4000-8000-000000000002".to_owned(),
                origin_kind: origin_kind.to_owned(),
                origin_id: "origin".to_owned(),
                recipient: "MoogleMail".to_owned(),
                cod: None,
                status: "pending".to_owned(),
                template: None,
                category: None,
                world: Some("main".to_owned()),
                retirement_reason: None,
                suppression_reason: None,
                lease_expires_at: None,
                fencing_version: 1,
                attempt_count: 0,
                claimed_at: None,
                sending_at: None,
                completed_at: None,
                external_reference: None,
                failure_code: None,
                reason_code: None,
                take_required: false,
                returned_mail_id: None,
                created_at: "2026-11-16T00:00:00Z".to_owned(),
                updated_at: "2026-11-16T00:00:00Z".to_owned(),
            },
            attempts: Vec::new(),
            evidence: Vec::new(),
            custody: AdminDeliveryCustodyResponse {
                collected: None,
                cod_pending: None,
                remaining_credits: None,
                receipt: None,
                observed_at: None,
                operator_confirmed_collected: false,
            },
            dispositions: DeliveryDispositionKind::ALL
                .into_iter()
                .map(DeliveryDispositionKind::as_str)
                .chain(["release", "retry_failed", "reconcile", "sent_operator_confirmed"])
                .map(disposition)
                .collect(),
            obligation: None,
            recovery: None,
        }
    }

    async fn error_body(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
        (status, serde_json::from_slice(&bytes).expect("错误响应是 JSON"))
    }

    /// 缺省详情要么能被升级前的旧 decoder 按闭集解码（来源与每个处置种类都在旧闭集内），要么
    /// 以 404 `delivery_projection_excluded` 明确拒绝；被拒绝的恰好是结算退件 Take。
    #[tokio::test]
    async fn default_detail_stays_decodable_by_the_released_page() {
        for origin in STORED_ORIGINS {
            match DeliveryProjection::Published.project_detail(detail(origin)) {
                Ok(projected) => {
                    assert!(
                        RELEASED_DECODER_ORIGINS.contains(&projected.delivery.origin_kind.as_str())
                    );
                    assert!(
                        projected
                            .dispositions
                            .iter()
                            .all(|item| RELEASED_DECODER_DISPOSITIONS.contains(&item.kind.as_str())),
                        "{origin}: {:?}",
                        projected.dispositions
                    );
                    assert_eq!(projected.dispositions.len(), RELEASED_DECODER_DISPOSITIONS.len());
                }
                Err(response) => {
                    assert_eq!(origin, "settlement_return_action");
                    let (status, body) = error_body(*response).await;
                    assert_eq!(status, StatusCode::NOT_FOUND);
                    assert_eq!(body["code"], "delivery_projection_excluded");
                }
            }
        }
    }

    /// 扩展投影读得到结算退件 Take 投递，并在已发布闭集之外只追加装备收回处置。
    #[test]
    fn settlement_returns_projection_adds_only_this_release_values() {
        for origin in STORED_ORIGINS {
            let projected = DeliveryProjection::SettlementReturns
                .project_detail(detail(origin))
                .expect("扩展投影输出全部来源");
            assert_eq!(projected.delivery.origin_kind, origin);
            assert_eq!(projected.dispositions.len(), RELEASED_DECODER_DISPOSITIONS.len() + 1);
            assert!(
                projected.dispositions.iter().all(|item| {
                    RELEASED_DECODER_DISPOSITIONS.contains(&item.kind.as_str())
                        || item.kind == "recovery_equipment_recovered"
                }),
                "{:?}",
                projected.dispositions
            );
        }
        assert!(DeliveryProjection::SettlementReturns.includes_settlement_return_actions());
        assert!(!DeliveryProjection::Published.includes_settlement_return_actions());
    }

    /// 被恢复终结的原结算投递（`failed/recovery_superseded`，没有发送时间）不进入缺省详情，扩展
    /// 投影照常输出；同来源的普通失败不受影响。
    #[tokio::test]
    async fn recovery_superseded_settlement_delivery_is_extended_only() {
        let mut superseded = detail("settlement");
        superseded.delivery.status = "failed".to_owned();
        superseded.delivery.failure_code = Some("recovery_superseded".to_owned());
        let response = DeliveryProjection::Published
            .project_detail(superseded.clone())
            .expect_err("缺省投影不输出被恢复终结的投递");
        let (status, body) = error_body(*response).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "delivery_projection_excluded");
        assert!(DeliveryProjection::SettlementReturns.project_detail(superseded.clone()).is_ok());
        superseded.delivery.failure_code = Some("send_rejected".to_owned());
        assert!(DeliveryProjection::Published.project_detail(superseded).is_ok());
    }

    /// 只有精确的 `settlement_returns` 是扩展投影；空值、大小写不同、逗号列表与旧的单项取值都以
    /// 400 `invalid_query` 拒绝，不静默退回缺省投影。
    #[tokio::test]
    async fn include_is_opt_in_and_fails_closed() {
        assert!(matches!(DeliveryProjection::parse(None), Ok(DeliveryProjection::Published)));
        assert!(matches!(
            DeliveryProjection::parse(Some("settlement_returns")),
            Ok(DeliveryProjection::SettlementReturns)
        ));
        for include in [
            "",
            "SETTLEMENT_RETURNS",
            "settlement_returns,recovery_equipment_recovered",
            "recovery_equipment_recovered",
            "settlement_return_action",
        ] {
            let response = DeliveryProjection::parse(Some(include)).expect_err(include);
            let (status, body) = error_body(*response).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{include}");
            assert_eq!(body["code"], "invalid_query", "{include}");
            assert_eq!(body["message"], "附加字段无效", "{include}");
        }
    }
}
