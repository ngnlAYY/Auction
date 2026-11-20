// 管理端跨页面展示辅助组件。

import { baseApiErrorMessage } from "@auction/web-shared/net/errors";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { currentLanguage, formatLocalizedTimeForLanguage, t } from "@auction/web-shared/ui/i18n";
import type { Accessor } from "solid-js";
import { createEffect, onCleanup, untrack } from "solid-js";

import { adminApiFailureDisplayCode } from "../api/failure";

/**
 * 直接订阅当前 I18n 语言，避免 `<html lang>` 的同步 effect 使时间文案滞后一轮。
 * 时区固定为 Asia/Shanghai，格式及非法时间回退由共享层拥有。
 */
export function formatAdminTime(value: string): string {
  return formatLocalizedTimeForLanguage(value, currentLanguage());
}

/**
 * 业务世界的当前语言展示名，全站列表、筛选项、表单选项与仪表盘的唯一来源；
 * 页面不得另建世界名译文键，未知值按原文显示。
 */
export function adminWorldLabel(worldId: string): string {
  switch (worldId) {
    case "main":
      return t("admin-runtime-world-main");
    case "isekai":
      return t("admin-runtime-world-isekai");
    default:
      return worldId;
  }
}

/** 拍品状态的当前语言展示名；拍品列表与玩家支持共用，未知值按原文显示。 */
export function lotStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return t("admin-lots-status-pending");
    case "live":
      return t("admin-lots-status-live");
    case "sold":
      return t("admin-lots-status-sold");
    case "unsold":
      return t("admin-lots-status-unsold");
    case "withdrawn":
      return t("admin-lots-status-withdrawn");
    default:
      return status;
  }
}

/**
 * 将 Admin API 的稳定错误码映射为当前语言的可操作提示（对应旧版 `errorKey(error)`）。
 * 底层 HTTP 正文、数据库错误和浏览器异常不直接展示，服务端自然语言不作为跨语言契约。
 * 分类顺序：先匹配 Admin 业务码（含被基础层归入 Unauthenticated 的 `invalid_credentials`），
 * 未命中再走共享基础层 `baseApiErrorMessage` 的通用码 / `http_*` 兜底，最后落到通用兜底译文。
 */
export function adminApiErrorMessage(code: string): string {
  const displayCode = adminApiFailureDisplayCode(code);
  switch (displayCode) {
    case "admin_pagination_bounds":
      return t("admin-pagination-bounds");
    case "alert_stale":
      return t("alert-console-stale");
    case "body_too_large":
      return t("admin-error-body-too-large");
    case "invalid_auction_command":
      return t("admin-error-invalid-auction-command");
    case "invalid_auction_state":
      return t("admin-error-invalid-auction-state");
    case "invalid_auction_intake":
      return t("admin-error-invalid-auction-intake");
    case "unsupported_equipment_source":
      return t("admin-error-unsupported-equipment-source");
    case "equipment_source_unavailable":
      return t("admin-error-equipment-source-unavailable");
    case "auction_intake_unavailable":
      return t("admin-error-auction-intake-unavailable");
    case "seller_blacklisted":
    case "auction_intake_player_blacklisted":
      return t("admin-error-auction-intake-player-blacklisted");
    case "equipment_conflict":
      return t("admin-error-equipment-conflict");
    case "invalid_credentials":
      return t("admin-error-invalid-credentials");
    case "invalid_credential_secret":
      return t("admin-credentials-field-secret-invalid");
    case "credential_login_rejected":
      return t("admin-error-credential-login-rejected");
    case "credential_login_challenge":
      return t("admin-error-credential-login-challenge");
    case "credential_login_unavailable":
      return t("admin-error-credential-login-unavailable");
    case "credential_login_busy":
      return t("admin-error-credential-login-busy");
    case "manual_early_settlement_disabled":
      return t("admin-error-manual-early-settlement-disabled");
    case "world_has_active_auction":
      return t("admin-error-world-has-active-auction");
    case "world_not_found":
      return t("admin-error-world-not-found");
    case "invalid_mail_template":
      return t("admin-error-invalid-mail-template");
    case "invalid_delivery_id":
      return t("admin-error-invalid-delivery-id");
    case "invalid_delivery_command":
      return t("admin-error-invalid-delivery-command");
    case "stale_delivery_state":
      return t("admin-error-stale-delivery-state");
    case "delivery_sent_sighted":
      return t("admin-error-delivery-sent-sighted");
    case "settlement_obligation_ineligible":
      return t("admin-error-settlement-obligation-ineligible");
    case "recovery_send_unattributed":
      return t("admin-error-recovery-send-unattributed");
    case "invalid_settlement_return_command":
      return t("admin-error-invalid-settlement-return-command");
    case "invalid_settlement_return_id":
      return t("admin-error-invalid-settlement-return-id");
    case "settlement_return_not_found":
      return t("admin-error-settlement-return-not-found");
    case "stale_settlement_return_state":
      return t("admin-error-stale-settlement-return-state");
    case "settlement_return_untrusted":
      return t("admin-error-settlement-return-untrusted");
    case "settlement_return_unbound":
      return t("admin-error-settlement-return-unbound");
    case "settlement_return_assets_mismatch":
      return t("admin-error-settlement-return-assets-mismatch");
    case "settlement_return_action_conflict":
      return t("admin-error-settlement-return-action-conflict");
    case "settlement_return_proof_required":
      return t("admin-error-settlement-return-proof-required");
    case "settlement_return_resend_limit":
      return t("admin-error-settlement-return-resend-limit");
    case "settlement_return_obligation_ineligible":
      return t("admin-error-settlement-return-obligation-ineligible");
    case "settlement_return_send_unresolved":
      return t("admin-error-settlement-return-send-unresolved");
    case "settlement_return_observation_required":
      return t("admin-error-settlement-return-observation-required");
    case "delivery_failed":
      return t("admin-error-delivery-failed");
    case "delivery_backlog":
      return t("admin-error-delivery-backlog");
    case "compose_not_clean":
      return t("admin-error-compose-not-clean");
    case "preflight_unproven":
      return t("admin-error-preflight-unproven");
    case "recipient_is_sender":
      return t("admin-error-recipient-is-sender");
    case "write_unknown":
      return t("admin-error-write-unknown");
    case "reconciliation_required":
    case "outcome_unknown":
      return t("admin-error-reconciliation-required");
    // 这三个业务码带冲突语义但不命中基础层的 `*_conflict` / `stale_*` 模式
    // （`invalid_lifecycle` 反而命中基础层 `invalid_*`），必须留在业务分支。
    case "recovery_not_confirmable":
    case "recovery_evidence_unresolved":
    case "invalid_lifecycle":
      return t("admin-error-conflict");
    default:
      break;
  }
  const known = baseApiErrorMessage(displayCode, (kind) => {
    switch (kind) {
      case "network":
        return t("admin-error-network");
      case "timeout":
        return t("admin-error-request-timeout");
      case "invalid_response":
        return t("admin-error-invalid-response");
      case "unauthenticated":
        return t("admin-error-unauthenticated");
      case "forbidden":
        return t("admin-error-forbidden");
      case "not_found":
        return t("admin-error-not-found");
      case "rate_limited":
        return t("admin-error-rate-limited");
      case "unavailable":
        return t("admin-error-storage-unavailable");
      case "feature_not_supported":
        return t("admin-error-feature-not-supported");
      case "conflict":
        return t("admin-error-conflict");
      case "invalid_request":
        return t("admin-error-invalid-request");
      default:
        return unreachable(kind);
    }
  });
  return known ?? t("admin-error-generic");
}

/** 管理动作统一反馈；成功与稳定错误码不能再压缩为一个布尔值。 */
export type AdminActionFeedback =
  | { readonly kind: "success" }
  | { readonly kind: "error"; readonly code: string };

/** 是否为成功反馈。 */
export function adminActionFeedbackIsSuccess(feedback: AdminActionFeedback): boolean {
  return feedback.kind === "success";
}

/** 成功时返回调用方提供的文案，失败时把稳定错误码映射为当前语言提示。 */
export function adminActionFeedbackMessage(feedback: AdminActionFeedback, success: string): string {
  return feedback.kind === "success" ? success : adminApiErrorMessage(feedback.code);
}

interface UnloadWarning {
  readonly window: Window;
  readonly callback: (event: BeforeUnloadEvent) => void;
  /** 移除监听前先失效；移除失败时旧回调也不会继续拦截卸载。 */
  active: boolean;
}

/** 有未保存的会话草稿时请求浏览器原生确认；同文档切页不触发此事件。 */
export function useUnloadWarning(dirty: Accessor<boolean>): void {
  let warning: UnloadWarning | null = null;
  const clearWarning = (): void => {
    const current = warning;
    if (current === null) {
      return;
    }
    // 先清空守卫并让旧回调失效，再从 DOM 移除；移除抛错不能阻断组件销毁。
    warning = null;
    current.active = false;
    try {
      current.window.removeEventListener("beforeunload", current.callback);
    } catch {
      // Rust 的 Drop 忽略移除失败，这里同样只做尽力清理。
    }
  };
  onCleanup(clearWarning);
  // 源码接收的是按值去重的 `Memo<bool>`；等值通知不重建监听。
  const dirtyValue = createValueMemo(dirty);
  createEffect(dirtyValue, (isDirty) => {
    if (!isDirty) {
      clearWarning();
      return;
    }
    if (warning !== null) {
      return;
    }
    if (typeof window === "undefined") {
      return;
    }
    const currentWindow = window;
    const entry: UnloadWarning = {
      window: currentWindow,
      active: true,
      callback: (event: BeforeUnloadEvent): void => {
        if (!entry.active) {
          return;
        }
        // 事件触发时再读取最新值，避免沿用安装监听时的旧快照。
        if (untrack(dirty)) {
          event.preventDefault();
          // 提示文本由浏览器控制；非空 returnValue 兼容仍依赖该属性的浏览器。
          event.returnValue = " ";
        }
      },
    };
    try {
      currentWindow.addEventListener("beforeunload", entry.callback);
    } catch {
      // 源码在监听安装失败时不保留守卫，避免卸载时移除一个不存在的监听。
      return;
    }
    warning = entry;
  });
}
