//! 邮件审查的本地化说明；只消费安全代码，不改业务结果、重试策略或持久化诊断。

use auction_web_shared::legacy_t as t;

/// 显示已有处理决定与业务原因；未知代码保留原值，不推测失败原因。
pub(super) fn processing_code_message(code: &str) -> String {
    if code.is_empty() {
        return t!("value-empty");
    }
    let message = match code {
        "auction_equipment_rejected" => t!("admin-mail-result-equipment-rejected"),
        "auction_assets_rejected" => t!("admin-mail-result-assets-rejected"),
        "auction_no_attachments" => t!("admin-mail-result-no-attachments"),
        "auction_accepted" => t!("admin-mail-result-auction-accepted"),
        "unknown_assets_rejected" => t!("admin-mail-result-unknown-assets"),
        "user_assets_rejected" => t!("admin-mail-result-user-assets"),
        "registration_accepted" => t!("admin-mail-result-registration-accepted"),
        "registration_rejected" => t!("admin-mail-result-registration-rejected"),
        "rebind_accepted" => t!("admin-mail-result-rebind-accepted"),
        "rebind_rejected" => t!("admin-mail-result-rebind-rejected"),
        "rekey_accepted" => t!("admin-mail-result-rekey-accepted"),
        "rekey_rejected" => t!("admin-mail-result-rekey-rejected"),
        "archived" => t!("admin-mail-result-archived"),
        "settlement_recovery_returned" => t!("admin-mail-result-recovery-returned"),
        "start_price_invalid" => t!("admin-mail-reason-price-invalid"),
        "start_price_missing" => t!("admin-mail-reason-price-missing"),
        "start_price_duplicate" => t!("admin-mail-reason-price-duplicate"),
        "start_price_identity_mismatch" => t!("admin-mail-reason-price-identity"),
        "start_price_attachment_not_found" => t!("admin-mail-reason-price-attachment"),
        "season_intake_closed" => t!("admin-mail-reason-season-intake-closed"),
        _ => detail_message(code).unwrap_or_else(|| t!("admin-mail-code-unknown")),
    };
    with_code(code, message)
}

/// 装备读取的闭集分类说明，不把响应失败或快照错误归咎于装备品质。
pub(super) fn equipment_category_message(code: &str) -> String {
    let message = match code {
        "configuration" => t!("admin-mail-category-configuration"),
        "credential_unavailable" => t!("admin-mail-category-credential-unavailable"),
        "session_expired" => t!("admin-mail-category-session-expired"),
        "timeout" => t!("admin-mail-category-timeout"),
        "network" => t!("admin-mail-category-network"),
        "redirect" => t!("admin-mail-category-redirect"),
        "response_too_large" => t!("admin-mail-category-response-too-large"),
        "content_encoding" => t!("admin-mail-category-content-encoding"),
        "rate_limited" => t!("admin-mail-category-rate-limited"),
        "server_error" => t!("admin-mail-category-server-error"),
        "http_status" => t!("admin-mail-category-http-status"),
        "content_type" => t!("admin-mail-category-content-type"),
        "protocol_drift" => t!("admin-mail-category-protocol-drift"),
        "identity_mismatch" => t!("admin-mail-category-identity-mismatch"),
        "snapshot_invalid" => t!("admin-mail-category-snapshot-invalid"),
        "filter_rejected" => t!("admin-mail-category-filter-rejected"),
        _ => t!("admin-mail-code-unknown"),
    };
    with_code(code, message)
}

/// 详细原因始终与原始代码一起显示，便于与 Worker 日志逐字关联。
pub(super) fn equipment_detail_message(code: &str) -> String {
    with_code(code, detail_message(code).unwrap_or_else(|| t!("admin-mail-code-unknown")))
}

/// 提示核对方向，不把查看页面或修正配置表述为自动重放、重新接收或完成退回。
pub(super) fn equipment_guidance(category: &str, detail: Option<&str>) -> String {
    if category == "snapshot_invalid" && detail == Some("collected_at_future") {
        return t!("admin-mail-guidance-collected-at-future");
    }
    match category {
        "credential_unavailable" | "session_expired" => t!("admin-mail-guidance-credential"),
        "timeout" | "network" | "rate_limited" | "server_error" => {
            t!("admin-mail-guidance-transport")
        }
        "filter_rejected" => t!("admin-mail-guidance-filter"),
        _ => t!("admin-mail-guidance-investigate"),
    }
}

/// 证据认证、加密和留存失败属于诊断文件，不能误报为游戏登录或装备接收失败。
pub(super) fn evidence_error_message(code: &str) -> String {
    let message = match code {
        "invalid_context" | "invalid_outcome" | "invalid_message" | "serialization" => {
            t!("admin-mail-evidence-error-format")
        }
        "payload_too_large" | "capacity" => t!("admin-mail-evidence-error-capacity"),
        "body_unavailable" => t!("admin-mail-evidence-error-body"),
        "encryption" => t!("admin-mail-evidence-error-encryption"),
        "authentication" => t!("admin-mail-evidence-error-authentication"),
        "io" => t!("admin-mail-evidence-error-io"),
        "not_found" => t!("admin-mail-evidence-error-not-found"),
        "unsupported_platform" => t!("admin-mail-evidence-error-platform"),
        "retention_timeout" | "retention_cancelled" => t!("admin-mail-evidence-error-unconfirmed"),
        "recorder_unavailable" => t!("admin-mail-evidence-error-recorder"),
        _ => t!("admin-mail-code-unknown"),
    };
    with_code(code, message)
}

fn detail_message(code: &str) -> Option<String> {
    Some(match code {
        "equipment_name_invalid" => t!("admin-mail-detail-source-name-invalid"),
        "equipment_quality_unknown" => t!("admin-mail-detail-source-quality-unknown"),
        "equipment_kind_unknown" => t!("admin-mail-detail-source-kind-unknown"),
        "equipment_category_unknown" => t!("admin-mail-detail-source-category-unknown"),
        "equipment_category_mismatch" => t!("admin-mail-detail-source-category-mismatch"),
        "equipment_level_invalid" => t!("admin-mail-detail-source-level-invalid"),
        "equipment_page_structure_invalid" => t!("admin-mail-detail-source-page-structure-invalid"),
        "equipment_attributes_invalid" => t!("admin-mail-detail-source-attributes-invalid"),
        "equipment_canonical_mismatch" => t!("admin-mail-detail-source-canonical-mismatch"),
        "adapter_invalid" => t!("admin-mail-detail-adapter-invalid"),
        "fingerprint_invalid" => t!("admin-mail-detail-fingerprint-invalid"),
        "equipment_id_invalid" => t!("admin-mail-detail-equipment-id-invalid"),
        "name_invalid" => t!("admin-mail-detail-name-invalid"),
        "category_invalid" => t!("admin-mail-detail-category-invalid"),
        "level_invalid" => t!("admin-mail-detail-level-invalid"),
        "collected_at_invalid" => t!("admin-mail-detail-collected-at-invalid"),
        "collected_at_future" => t!("admin-mail-detail-collected-at-future"),
        "attributes_invalid" => t!("admin-mail-detail-attributes-invalid"),
        "quality_not_allowed" => t!("admin-mail-detail-quality-not-allowed"),
        "prefix_not_allowed" => t!("admin-mail-detail-prefix-not-allowed"),
        "suffix_not_allowed" => t!("admin-mail-detail-suffix-not-allowed"),
        "category_not_allowed" => t!("admin-mail-detail-category-not-allowed"),
        "equipment_type_not_allowed" => t!("admin-mail-detail-equipment-type-not-allowed"),
        "level_not_allowed" => t!("admin-mail-detail-level-not-allowed"),
        "tradeable_required" => t!("admin-mail-detail-tradeable-required"),
        _ => return None,
    })
}

fn with_code(code: &str, message: String) -> String {
    t!("admin-mail-explanation-with-code", message: message, code: code)
}
