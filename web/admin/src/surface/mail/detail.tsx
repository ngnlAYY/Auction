// Admin 邮件入站详情安全投影。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show } from "solid-js";
import type { AdminMailIntakeDetail } from "../../api/mail";
import {
  type AdminActionFeedback,
  adminApiErrorMessage,
  adminWorldLabel,
  formatAdminTime,
} from "../presentation";
import { MailDeliveriesPanel } from "./deliveries";
import { mailKindLabel, mailStatusClass, mailStatusLabel } from "./labels";
import {
  equipmentCategoryMessage,
  equipmentDetailMessage,
  equipmentGuidance,
  evidenceErrorMessage,
  processingCodeMessage,
} from "./messages";

const U64_MAX = 18_446_744_073_709_551_615n;

/** 邮件详情面板属性；`visible` 为假时不渲染任何节点。 */
export interface MailIntakeDetailPanelProps {
  readonly visible: boolean;
  readonly detail: AdminMailIntakeDetail | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly refresh: Cell<bigint>;
  readonly deliveryNotice: Cell<readonly [string, AdminActionFeedback] | null>;
  readonly onClose: () => void;
}

/** 入站详情面板；加载、错误与空选择各自复用共享异步状态组件。 */
export function MailIntakeDetailPanel(props: MailIntakeDetailPanelProps): JSX.Element {
  // 投递失败反馈归邮件页所有：切换收件只重建投递面板，不能把页级错误一起丢弃。
  let pageAlive = true;
  onCleanup(() => {
    pageAlive = false;
  });
  const nextRefresh = (): void => {
    const current = props.refresh();
    props.refresh.set(current === U64_MAX ? U64_MAX : current + 1n);
  };
  return (
    <Show when={props.visible}>
      <Show
        when={!props.loading}
        fallback={
          <AsyncState
            kind="loading"
            title={t("admin-mail-detail-title")}
            detail={t("admin-mail-detail-loading")}
          />
        }
      >
        <Show
          when={props.error === null}
          fallback={
            <>
              <AsyncState
                kind="error"
                title={t("admin-mail-detail-title")}
                detail={adminApiErrorMessage(props.error ?? "")}
                retryLabel={t("admin-retry")}
                onRetry={nextRefresh}
              />
              {/* 共享面板只承载一个重试按钮，关闭入口单独保留，行为与旧版一致。 */}
              <button type="button" onClick={() => props.onClose()}>
                {t("admin-mail-close-detail")}
              </button>
            </>
          }
        >
          <Show
            when={props.detail}
            fallback={
              <AsyncState
                kind="empty"
                title={t("admin-mail-detail-title")}
                detail={t("admin-mail-no-selection")}
              />
            }
          >
            {(detail) => (
              <MailIntakeDetailContent
                detail={detail()}
                refresh={props.refresh}
                deliveryNotice={props.deliveryNotice}
                pageAlive={() => pageAlive}
                onClose={props.onClose}
              />
            )}
          </Show>
        </Show>
      </Show>
    </Show>
  );
}

interface MailIntakeDetailContentProps {
  readonly detail: AdminMailIntakeDetail;
  readonly refresh: Cell<bigint>;
  readonly deliveryNotice: Cell<readonly [string, AdminActionFeedback] | null>;
  readonly pageAlive: () => boolean;
  readonly onClose: () => void;
}

/** 已解码的详情内容；所有展示值只来自安全投影。 */
function MailIntakeDetailContent(props: MailIntakeDetailContentProps): JSX.Element {
  const detail = () => props.detail;
  const metadata = () => props.detail.metadata;
  const warnings = (): string =>
    metadata().warning_codes.length === 0 ? t("value-empty") : metadata().warning_codes.join(", ");
  const deliveryIds = (): string =>
    metadata().delivery_ids.length === 0 ? t("value-empty") : metadata().delivery_ids.join(", ");
  const original = () => detail().message_display;
  const equipmentDiagnostic = () => detail().equipment_diagnostic;
  const decisionMessage = (): string => processingCodeMessage(detail().decision_code);
  const reasonMessage = (): string => processingCodeMessage(metadata().reason_code);
  // 优先使用已持久化的具体诊断；列表总码或 HTTP 200 都不能推导登录态、品质或退回结果。
  const rejectionSummary = (): string | null => {
    if (detail().decision_code !== "auction_equipment_rejected") {
      return null;
    }
    const diagnostic = equipmentDiagnostic();
    if (diagnostic !== null) {
      const code = diagnostic.detail_code;
      return code === null
        ? equipmentCategoryMessage(diagnostic.category)
        : equipmentDetailMessage(code);
    }
    if (metadata().reason_code !== "" && metadata().reason_code !== detail().decision_code) {
      return processingCodeMessage(metadata().reason_code);
    }
    return t("admin-mail-rejection-cause-unavailable");
  };
  const rejectionGuidance = (): string => {
    const diagnostic = equipmentDiagnostic();
    return diagnostic === null
      ? t("admin-mail-guidance-investigate")
      : equipmentGuidance(diagnostic.category, diagnostic.detail_code);
  };
  const sender = (): string => detail().sender ?? t("admin-mail-original-not-retained");
  const subject = (): string => detail().subject ?? t("admin-mail-original-not-retained");
  const diagnosticDetail = (): string => {
    const code = equipmentDiagnostic()?.detail_code ?? null;
    return code === null ? t("value-empty") : equipmentDetailMessage(code);
  };
  const diagnosticHttpStatus = (): string => {
    const value = equipmentDiagnostic()?.http_status ?? null;
    return value === null ? t("value-empty") : String(value);
  };
  const diagnosticElapsed = (): string => {
    const value = equipmentDiagnostic()?.elapsed_millis ?? null;
    return value === null ? t("value-empty") : String(value);
  };
  const diagnosticEvidenceId = (): string => equipmentDiagnostic()?.evidence_id ?? t("value-empty");
  const diagnosticRetentionError = (): string => {
    const value = equipmentDiagnostic()?.retention_error;
    return value === null || value === undefined ? t("value-empty") : evidenceErrorMessage(value);
  };
  const diagnosticStage = (): string => {
    const diagnostic = equipmentDiagnostic();
    return diagnostic === null ? t("value-empty") : equipmentDiagnosticStageLabel(diagnostic.stage);
  };
  const diagnosticEvidenceStatus = (): string => {
    const diagnostic = equipmentDiagnostic();
    return diagnostic === null
      ? t("value-empty")
      : equipmentEvidenceStatusLabel(diagnostic.evidence_status);
  };
  // 只重建当前收件的分页与未提交确认，已提交的投递处置仍归会话宿主。
  const deliveryPanel = createValueMemo(() => ({
    intakeId: props.detail.id,
    deliveries: props.detail.deliveries,
  }));
  return (
    <section class="admin-panel admin-mail-detail" aria-labelledby="admin-mail-detail-title">
      <header class="admin-mail-detail__header">
        <h2 id="admin-mail-detail-title">{t("admin-mail-detail-title")}</h2>
        <button type="button" onClick={() => props.onClose()}>
          {t("admin-mail-close-detail")}
        </button>
      </header>
      <Show when={rejectionSummary() !== null}>
        <section
          class="admin-mail-rejection-summary"
          aria-labelledby="admin-mail-rejection-summary-title"
        >
          <h3 id="admin-mail-rejection-summary-title">{t("admin-mail-rejection-summary-title")}</h3>
          <p>{rejectionSummary() ?? ""}</p>
          <p class="admin-panel__note">{rejectionGuidance()}</p>
          <p class="admin-panel__note">{t("admin-mail-rejection-delivery-boundary")}</p>
        </section>
      </Show>
      <dl class="admin-mail-detail__grid">
        <dt>{t("admin-mail-intake-id")}</dt>
        <dd>{detail().id}</dd>
        <dt>{t("admin-mail-external-id")}</dt>
        <dd>{detail().external_mail_id}</dd>
        <dt>{t("admin-mail-mailbox")}</dt>
        <dd>{detail().mailbox_key}</dd>
        <dt>{t("admin-mail-intake-from")}</dt>
        <dd>{sender()}</dd>
        <dt>{t("admin-mail-intake-subject")}</dt>
        <dd>{subject()}</dd>
        <dt>{t("admin-mail-intake-world")}</dt>
        <dd>{adminWorldLabel(detail().world_id)}</dd>
        <dt>{t("admin-mail-intake-kind")}</dt>
        <dd>{mailKindLabel(detail().kind)}</dd>
        <dt>{t("admin-mail-intake-status")}</dt>
        <dd>
          <span class={mailStatusClass(detail().status)}>{mailStatusLabel(detail().status)}</span>
        </dd>
        <dt>{t("admin-mail-intake-decision")}</dt>
        <dd>{decisionMessage()}</dd>
        <dt>{t("admin-mail-intake-received")}</dt>
        <dd>{formatAdminTime(detail().received_at)}</dd>
        <dt>{t("admin-mail-processed")}</dt>
        <dd>{formatAdminTime(detail().processed_at)}</dd>
        <dt>{t("admin-mail-auction-id")}</dt>
        <dd>{metadata().auction_id}</dd>
        <dt>{t("admin-mail-submission-id")}</dt>
        <dd>{metadata().submission_id}</dd>
        <dt>{t("admin-mail-user-id")}</dt>
        <dd>{metadata().user_id}</dd>
        <dt>{t("admin-mail-added-count")}</dt>
        <dd>{metadata().added_count}</dd>
        <dt>{t("admin-mail-existing-count")}</dt>
        <dd>{metadata().existing_count}</dd>
        <dt>{t("admin-mail-preview-revision")}</dt>
        <dd>{metadata().preview_revision}</dd>
        <dt>{t("admin-mail-attachment-count")}</dt>
        <dd>{metadata().attachment_count}</dd>
        <dt>{t("admin-mail-owner-result")}</dt>
        <dd>{metadata().owner_result_code}</dd>
        <dt>{t("admin-mail-reason-code")}</dt>
        <dd>{reasonMessage()}</dd>
        <dt>{t("admin-mail-warnings")}</dt>
        <dd>{warnings()}</dd>
        <dt>{t("admin-mail-delivery-ids")}</dt>
        <dd>{deliveryIds()}</dd>
      </dl>
      <For each={[deliveryPanel()]} keyed={(panel) => panel.intakeId}>
        {(panel) => (
          <MailDeliveriesPanel
            intakeId={panel().intakeId}
            deliveries={panel().deliveries}
            notice={props.deliveryNotice}
            pageAlive={props.pageAlive}
            refresh={props.refresh}
          />
        )}
      </For>
      <section class="admin-mail-original" aria-labelledby="admin-mail-original-title">
        <h3 id="admin-mail-original-title">{t("admin-mail-original")}</h3>
        <Show
          when={original()}
          fallback={
            <p class="admin-mail-original__missing">{t("admin-mail-original-not-retained")}</p>
          }
        >
          {(current) => (
            <>
              <dl class="admin-mail-original__grid">
                <dt>{t("admin-mail-original-body")}</dt>
                <dd>
                  <pre class="admin-mail-original__body">{current().body}</pre>
                </dd>
                <dt>{t("admin-mail-original-attachments")}</dt>
                <dd>
                  <Show when={current().attachment_ids.length > 0} fallback={t("value-empty")}>
                    <ul class="admin-mail-original__attachments">
                      <For each={current().attachment_ids} keyed={false}>
                        {(attachmentId) => <li>{attachmentId()}</li>}
                      </For>
                    </ul>
                  </Show>
                </dd>
                <dt>{t("admin-mail-original-credits")}</dt>
                <dd>{current().credits}</dd>
                <dt>{t("admin-mail-original-cod")}</dt>
                <dd>{current().cod}</dd>
              </dl>
              <p class="admin-mail-original__note">{t("admin-mail-original-redacted")}</p>
            </>
          )}
        </Show>
      </section>
      <section
        class="admin-mail-equipment-diagnostic"
        aria-labelledby="admin-mail-equipment-diagnostic-title"
      >
        <h3 id="admin-mail-equipment-diagnostic-title">{t("admin-mail-equipment-diagnostic")}</h3>
        <Show
          when={equipmentDiagnostic()}
          fallback={
            <p class="admin-mail-equipment-diagnostic__missing">
              {t("admin-mail-equipment-diagnostic-not-retained")}
            </p>
          }
        >
          {(diagnostic) => (
            <>
              <dl class="admin-mail-equipment-diagnostic__grid">
                <dt>{t("admin-mail-equipment-id")}</dt>
                <dd>{diagnostic().equipment_id}</dd>
                <dt>{t("admin-mail-attachment-position")}</dt>
                <dd>{diagnostic().attachment_position}</dd>
                <dt>{t("admin-mail-diagnostic-stage")}</dt>
                <dd>{diagnosticStage()}</dd>
                <dt>{t("admin-mail-diagnostic-category")}</dt>
                <dd>{equipmentCategoryMessage(diagnostic().category)}</dd>
                <dt>{t("admin-mail-diagnostic-detail")}</dt>
                <dd>{diagnosticDetail()}</dd>
                <dt>{t("admin-mail-diagnostic-http-status")}</dt>
                <dd>{diagnosticHttpStatus()}</dd>
                <dt>{t("admin-mail-diagnostic-elapsed")}</dt>
                <dd>{diagnosticElapsed()}</dd>
                <dt>{t("admin-mail-diagnostic-evidence-id")}</dt>
                <dd>{diagnosticEvidenceId()}</dd>
                <dt>{t("admin-mail-diagnostic-evidence-status")}</dt>
                <dd>{diagnosticEvidenceStatus()}</dd>
                <dt>{t("admin-mail-diagnostic-retention-error")}</dt>
                <dd>{diagnosticRetentionError()}</dd>
              </dl>
              <p class="admin-panel__note">{t("admin-mail-evidence-boundary")}</p>
            </>
          )}
        </Show>
      </section>
    </section>
  );
}

/** 装备诊断阶段标签；未知值原样显示。 */
function equipmentDiagnosticStageLabel(value: string): string {
  switch (value) {
    case "request":
      return t("admin-mail-diagnostic-stage-request");
    case "response":
      return t("admin-mail-diagnostic-stage-response");
    case "parse":
      return t("admin-mail-diagnostic-stage-parse");
    case "validate":
      return t("admin-mail-diagnostic-stage-validate");
    default:
      return value;
  }
}

/** 装备诊断证据状态标签；未知值原样显示。 */
function equipmentEvidenceStatusLabel(value: string): string {
  switch (value) {
    case "retained":
      return t("admin-mail-diagnostic-evidence-retained");
    case "body_unavailable":
      return t("admin-mail-diagnostic-evidence-body-unavailable");
    case "save_failed":
      return t("admin-mail-diagnostic-evidence-save-failed");
    case "completion_unconfirmed":
      return t("admin-mail-diagnostic-evidence-completion-unconfirmed");
    case "prepare_failed":
      return t("admin-mail-diagnostic-evidence-prepare-failed");
    case "disabled":
      return t("admin-mail-diagnostic-evidence-disabled");
    default:
      return value;
  }
}
