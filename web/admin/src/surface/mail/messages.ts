// 邮件审查的本地化说明；只消费安全代码，不改业务结果、重试策略或持久化诊断。

import { t } from "@auction/web-shared/ui/i18n";

/** 显示已有处理决定与业务原因；未知代码保留原值，不推测失败原因。 */
export function processingCodeMessage(code: string): string {
  if (code === "") {
    return t("value-empty");
  }
  const message = (() => {
    switch (code) {
      case "auction_equipment_rejected":
        return t("admin-mail-result-equipment-rejected");
      case "auction_assets_rejected":
        return t("admin-mail-result-assets-rejected");
      case "auction_no_attachments":
        return t("admin-mail-result-no-attachments");
      case "auction_accepted":
        return t("admin-mail-result-auction-accepted");
      case "unknown_assets_rejected":
        return t("admin-mail-result-unknown-assets");
      case "user_assets_rejected":
        return t("admin-mail-result-user-assets");
      case "registration_accepted":
        return t("admin-mail-result-registration-accepted");
      case "registration_rejected":
        return t("admin-mail-result-registration-rejected");
      case "rebind_accepted":
        return t("admin-mail-result-rebind-accepted");
      case "rebind_rejected":
        return t("admin-mail-result-rebind-rejected");
      case "rekey_accepted":
        return t("admin-mail-result-rekey-accepted");
      case "rekey_rejected":
        return t("admin-mail-result-rekey-rejected");
      case "archived":
        return t("admin-mail-result-archived");
      case "settlement_recovery_returned":
        return t("admin-mail-result-recovery-returned");
      case "start_price_invalid":
        return t("admin-mail-reason-price-invalid");
      case "start_price_missing":
        return t("admin-mail-reason-price-missing");
      case "start_price_duplicate":
        return t("admin-mail-reason-price-duplicate");
      case "start_price_identity_mismatch":
        return t("admin-mail-reason-price-identity");
      case "start_price_attachment_not_found":
        return t("admin-mail-reason-price-attachment");
      case "season_intake_closed":
        return t("admin-mail-reason-season-intake-closed");
      case "auction_equipment_conflict":
        return t("admin-mail-reason-equipment-conflict");
      default:
        return detailMessage(code) ?? t("admin-mail-code-unknown");
    }
  })();
  return withCode(code, message);
}

/** 装备读取的闭集分类说明，不把响应失败或快照错误归咎于装备品质。 */
export function equipmentCategoryMessage(code: string): string {
  const message = (() => {
    switch (code) {
      case "configuration":
        return t("admin-mail-category-configuration");
      case "credential_unavailable":
        return t("admin-mail-category-credential-unavailable");
      case "session_expired":
        return t("admin-mail-category-session-expired");
      case "timeout":
        return t("admin-mail-category-timeout");
      case "network":
        return t("admin-mail-category-network");
      case "redirect":
        return t("admin-mail-category-redirect");
      case "response_too_large":
        return t("admin-mail-category-response-too-large");
      case "content_encoding":
        return t("admin-mail-category-content-encoding");
      case "rate_limited":
        return t("admin-mail-category-rate-limited");
      case "server_error":
        return t("admin-mail-category-server-error");
      case "http_status":
        return t("admin-mail-category-http-status");
      case "content_type":
        return t("admin-mail-category-content-type");
      case "protocol_drift":
        return t("admin-mail-category-protocol-drift");
      case "identity_mismatch":
        return t("admin-mail-category-identity-mismatch");
      case "snapshot_invalid":
        return t("admin-mail-category-snapshot-invalid");
      case "filter_rejected":
        return t("admin-mail-category-filter-rejected");
      default:
        return t("admin-mail-code-unknown");
    }
  })();
  return withCode(code, message);
}

/** 详细原因始终与原始代码一起显示，便于与 Worker 日志逐字关联。 */
export function equipmentDetailMessage(code: string): string {
  return withCode(code, detailMessage(code) ?? t("admin-mail-code-unknown"));
}

/** 提示核对方向，不把查看页面或修正配置表述为自动重放、重新接收或完成退回。 */
export function equipmentGuidance(category: string, detail: string | null): string {
  if (category === "snapshot_invalid" && detail === "collected_at_future") {
    return t("admin-mail-guidance-collected-at-future");
  }
  switch (category) {
    case "credential_unavailable":
    case "session_expired":
      return t("admin-mail-guidance-credential");
    case "timeout":
    case "network":
    case "rate_limited":
    case "server_error":
      return t("admin-mail-guidance-transport");
    case "filter_rejected":
      return t("admin-mail-guidance-filter");
    default:
      return t("admin-mail-guidance-investigate");
  }
}

/** 证据认证、加密和留存失败属于诊断文件，不能误报为游戏登录或装备接收失败。 */
export function evidenceErrorMessage(code: string): string {
  const message = (() => {
    switch (code) {
      case "invalid_context":
      case "invalid_outcome":
      case "invalid_message":
      case "serialization":
        return t("admin-mail-evidence-error-format");
      case "payload_too_large":
      case "capacity":
        return t("admin-mail-evidence-error-capacity");
      case "body_unavailable":
        return t("admin-mail-evidence-error-body");
      case "encryption":
        return t("admin-mail-evidence-error-encryption");
      case "authentication":
        return t("admin-mail-evidence-error-authentication");
      case "io":
        return t("admin-mail-evidence-error-io");
      case "not_found":
        return t("admin-mail-evidence-error-not-found");
      case "unsupported_platform":
        return t("admin-mail-evidence-error-platform");
      case "retention_timeout":
      case "retention_cancelled":
        return t("admin-mail-evidence-error-unconfirmed");
      case "recorder_unavailable":
        return t("admin-mail-evidence-error-recorder");
      default:
        return t("admin-mail-code-unknown");
    }
  })();
  return withCode(code, message);
}

/** 具体诊断代码说明；未知代码返回空，由调用方决定是否使用通用前缀。 */
function detailMessage(code: string): string | null {
  switch (code) {
    case "equipment_name_invalid":
      return t("admin-mail-detail-source-name-invalid");
    case "equipment_quality_unknown":
      return t("admin-mail-detail-source-quality-unknown");
    case "equipment_kind_unknown":
      return t("admin-mail-detail-source-kind-unknown");
    case "equipment_category_unknown":
      return t("admin-mail-detail-source-category-unknown");
    case "equipment_category_mismatch":
      return t("admin-mail-detail-source-category-mismatch");
    case "equipment_level_invalid":
      return t("admin-mail-detail-source-level-invalid");
    case "equipment_page_structure_invalid":
      return t("admin-mail-detail-source-page-structure-invalid");
    case "equipment_attributes_invalid":
      return t("admin-mail-detail-source-attributes-invalid");
    case "equipment_canonical_mismatch":
      return t("admin-mail-detail-source-canonical-mismatch");
    case "adapter_invalid":
      return t("admin-mail-detail-adapter-invalid");
    case "fingerprint_invalid":
      return t("admin-mail-detail-fingerprint-invalid");
    case "equipment_id_invalid":
      return t("admin-mail-detail-equipment-id-invalid");
    case "name_invalid":
      return t("admin-mail-detail-name-invalid");
    case "category_invalid":
      return t("admin-mail-detail-category-invalid");
    case "level_invalid":
      return t("admin-mail-detail-level-invalid");
    case "collected_at_invalid":
      return t("admin-mail-detail-collected-at-invalid");
    case "collected_at_future":
      return t("admin-mail-detail-collected-at-future");
    case "attributes_invalid":
      return t("admin-mail-detail-attributes-invalid");
    case "quality_not_allowed":
      return t("admin-mail-detail-quality-not-allowed");
    case "prefix_not_allowed":
      return t("admin-mail-detail-prefix-not-allowed");
    case "suffix_not_allowed":
      return t("admin-mail-detail-suffix-not-allowed");
    case "category_not_allowed":
      return t("admin-mail-detail-category-not-allowed");
    case "equipment_type_not_allowed":
      return t("admin-mail-detail-equipment-type-not-allowed");
    case "level_not_allowed":
      return t("admin-mail-detail-level-not-allowed");
    case "tradeable_required":
      return t("admin-mail-detail-tradeable-required");
    default:
      return null;
  }
}

/** 原值与说明一起显示，便于核对稳定代码。 */
function withCode(code: string, message: string): string {
  return t("admin-mail-explanation-with-code", { message, code });
}
