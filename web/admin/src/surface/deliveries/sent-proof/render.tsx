// 人工 Sent 证明对话框的渲染与展示文案。

import {
  adminDeliveryEvidencePreCollectionEquipmentIds,
  adminDeliveryEvidenceRemainingEquipmentIds,
  type AdminDeliveryDetailResponse as DeliveryDetail,
  type AdminDeliveryEvidenceResponse as DeliveryEvidence,
} from "@auction/contracts/dto/admin/delivery";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show, untrack } from "solid-js";

import { useAdminSessionHost } from "../../auth/shell";
import { adminWorldLabel } from "../../presentation";
import { useFocusField } from "../../validation";
import { deliveryReasonError } from "../labels";
import { latestSendAttempt, selectableEvidence } from "./evidence";
import { PreviewPanel, runPreview, submitProof } from "./preview";
import {
  reviewedSentProofCanConfirm,
  type SentProofDraft,
  type SentProofIssue,
  type SentProofReview,
  type SentProofSubmission,
  sentProofDraftForEvidence,
  sentProofDraftWithEvidence,
  sentProofIssueFieldId,
  sentProofIssueMessage,
  sentProofReviewClear,
  sentProofReviewDefault,
} from "./state";
import { missingObservedIds } from "./validation";

/** 证据种类译文；未知种类原样显示。 */
export function evidenceKindLabel(kind: string): string {
  switch (kind) {
    case "mailbox_sweep":
      return t("delivery-detail-evidence-kind-mailbox-sweep");
    case "sent_reference_sighting":
      return t("delivery-detail-evidence-kind-sent-sighting");
    case "compose_discard":
      return t("delivery-detail-evidence-kind-compose-discard");
    case "sent_operator_confirmed":
      return t("delivery-detail-evidence-kind-operator-confirmed");
    default:
      return kind;
  }
}

/** 展示可以从详情证据中选用的人工证明对话框。 */
export function SentProofDialog(props: {
  readonly detail: DeliveryDetail;
  readonly busy: boolean;
  readonly blocked?: boolean;
  readonly onSubmit: (submission: SentProofSubmission) => void;
  readonly onCancel: () => void;
  readonly onStale: () => void;
}): JSX.Element {
  const host = useAdminSessionHost();
  const tasks = createTaskScope();
  const focusField = useFocusField();
  const detail = createValueMemo(() => props.detail);
  const candidates = createValueMemo<readonly DeliveryEvidence[]>(() =>
    selectableEvidence(detail()),
  );
  // 首证据只在挂载时预填一次；用 untrack 取得初始化快照，避免组件体顶层的响应式读取。
  const draft = createCell<SentProofDraft>(
    sentProofDraftForEvidence(untrack(() => candidates()[0] ?? null)),
  );
  const review = createCell<SentProofReview>(sentProofReviewDefault());
  createEffect(detail, () => {
    review.set(sentProofReviewClear(review.peek()));
  });

  const current = draft;
  const selected = createValueMemo(() => {
    const reference = current().evidenceReference;
    return candidates().find((item) => item.reference === reference) ?? null;
  });
  const attempt = createValueMemo(() => latestSendAttempt(detail()));
  const reviewState = createValueMemo(() => review());
  const localIssue = createValueMemo(() => {
    const outcome = reviewState().outcome;
    if (outcome === null || outcome.ok || outcome.error.kind !== "local") {
      return null;
    }
    return outcome.error.issue;
  });
  const fence = createValueMemo(() => detail().delivery.fencing_version);
  const reasonError = createValueMemo(() => deliveryReasonError(current().reason));
  const canConfirm = createValueMemo(() => {
    const outcome = reviewState().outcome;
    return (
      outcome?.ok &&
      reviewedSentProofCanConfirm(outcome.value, detail().delivery.id, fence()) &&
      reasonError() === null
    );
  });
  const blocked = createValueMemo(() => props.blocked === true);
  const disabled = createValueMemo(() => props.busy || blocked());
  const confirmLabel = createValueMemo(() =>
    props.busy ? t("admin-action-working") : t("delivery-sent-proof-confirm"),
  );
  const previewLabel = createValueMemo(() =>
    reviewState().pending ? t("delivery-sent-proof-previewing") : t("delivery-sent-proof-preview"),
  );

  const world = createValueMemo(() => {
    const currentAttempt = attempt();
    return currentAttempt === null ? null : adminWorldLabel(currentAttempt.world_id);
  });
  const mid = createValueMemo(() => selected()?.external_mail_id ?? null);
  const recipient = createValueMemo(() => selected()?.recipient ?? attempt()?.recipient ?? null);
  const subject = createValueMemo(() => selected()?.subject ?? attempt()?.subject ?? null);
  const originalDigest = createValueMemo(() => attempt()?.body_digest ?? null);
  const observedDigest = createValueMemo(() => selected()?.body_digest ?? null);
  const frozenIds = createValueMemo(() => attempt()?.equipment_ids ?? null);
  const observedIds = createValueMemo(() => {
    const evidence = selected();
    return evidence === null ? null : adminDeliveryEvidenceRemainingEquipmentIds(evidence);
  });
  const beforeCollection = createValueMemo(() => {
    const evidence = selected();
    return evidence === null ? null : adminDeliveryEvidencePreCollectionEquipmentIds(evidence);
  });
  const missingIds = createValueMemo(() => {
    const evidence = selected();
    const currentAttempt = attempt();
    return (
      evidence !== null &&
      currentAttempt !== null &&
      missingObservedIds(evidence, currentAttempt, current().collected)
    );
  });
  const idsMatch = createValueMemo(() => {
    const frozen = frozenIds();
    if (frozen === null) {
      return null;
    }
    const observed = observedIds();
    const before = beforeCollection();
    const observedForComparison =
      current().collected && frozen.length > 0
        ? (before ?? (observed !== null && observed.length > 0 ? observed : null))
        : observed;
    if (observedForComparison === null) {
      return null;
    }
    return (
      sameIds(observedForComparison, frozen) &&
      (before === null || sameIds(before, frozen)) &&
      (observed === null || observed.length === 0 || sameIds(observed, frozen))
    );
  });

  const triggerPreview = (): void => {
    runPreview(host, tasks, detail(), disabled(), draft, review, focusField, props.onStale);
  };

  return (
    <Dialog
      title={t("delivery-sent-proof-title")}
      closeLabel={t("delivery-cancel")}
      busy={props.busy}
      onClose={() => props.onCancel()}
    >
      <form
        class="admin-delivery-sent-proof"
        method="post"
        aria-describedby={blocked() ? "admin-sent-proof-occupied" : undefined}
        onSubmit={(event) => {
          event.preventDefault();
          if (disabled()) {
            return;
          }
          submitProof(detail().delivery.id, fence(), draft, review, props.onSubmit);
        }}
      >
        <p class="admin-panel__note">{t("delivery-sent-proof-help")}</p>
        <Show when={blocked()}>
          <p id="admin-sent-proof-occupied" class="admin-panel__note">
            {t("delivery-command-occupied")}
          </p>
        </Show>
        <label for="delivery-sent-evidence">{t("delivery-sent-proof-evidence")}</label>
        <select
          id="delivery-sent-evidence"
          name="evidence_reference"
          disabled={disabled()}
          aria-invalid={fieldIsInvalid(localIssue(), "delivery-sent-evidence") ? "true" : "false"}
          aria-describedby={
            missingIds()
              ? "delivery-sent-evidence-missing-ids"
              : (fieldErrorId(localIssue(), "delivery-sent-evidence") ?? undefined)
          }
          value={current().evidenceReference}
          onChange={(event) => {
            const nextReference = event.currentTarget.value;
            const evidence =
              detail().evidence.find((item) => item.reference === nextReference) ?? null;
            draft.set(sentProofDraftWithEvidence(draft.peek(), evidence));
            review.set(sentProofReviewClear(review.peek()));
          }}
        >
          <option value="" disabled selected={current().evidenceReference === ""}>
            {t("delivery-sent-proof-evidence-placeholder")}
          </option>
          <For each={candidates()} keyed={(item) => item.reference}>
            {(item) => (
              <option
                value={item().reference}
                selected={current().evidenceReference === item().reference}
              >
                {`${evidenceKindLabel(item().kind)} · ${item().external_mail_id ?? ""}`}
              </option>
            )}
          </For>
        </select>
        {fieldError(localIssue(), "delivery-sent-evidence")}
        <Show when={missingIds()}>
          <p id="delivery-sent-evidence-missing-ids" class="admin-panel__note">
            {t("delivery-sent-proof-missing-observed-ids")}
          </p>
        </Show>
        <dl
          class="admin-delivery-sent-proof__grid"
          id="delivery-sent-evidence-summary"
          tabindex="-1"
          aria-describedby={
            fieldErrorId(localIssue(), "delivery-sent-evidence-summary") ?? undefined
          }
        >
          <dt>{t("delivery-sent-proof-world")}</dt>
          <dd>{world() ?? t("value-empty")}</dd>
          <dt>{t("delivery-sent-proof-mid")}</dt>
          <dd>{mid() ?? t("value-empty")}</dd>
          <dt>{t("delivery-sent-proof-recipient")}</dt>
          <dd>{recipient() ?? t("value-empty")}</dd>
          <dt>{t("delivery-sent-proof-subject")}</dt>
          <dd>{subject() ?? t("value-empty")}</dd>
          <dt>{t("delivery-sent-proof-original-digest")}</dt>
          <dd>
            <code>{originalDigest() ?? t("value-empty")}</code>
          </dd>
          <dt>{t("delivery-sent-proof-observed-digest")}</dt>
          <dd>
            <code>{observedDigest() ?? t("value-empty")}</code>
          </dd>
        </dl>
        {fieldError(localIssue(), "delivery-sent-evidence-summary")}
        <div class="admin-delivery-sent-proof__grid">
          <dl>
            <dt>{t("delivery-sent-proof-frozen-ids")}</dt>
            <dd>{equipmentIdsText(frozenIds())}</dd>
          </dl>
          <dl>
            <dt>{t("delivery-sent-proof-observed-ids")}</dt>
            <dd>{equipmentIdsText(observedIds())}</dd>
            <Show when={current().collected || beforeCollection() !== null}>
              <dt>{t("delivery-sent-proof-pre-collection-ids")}</dt>
              <dd>{equipmentIdsText(beforeCollection())}</dd>
            </Show>
          </dl>
        </div>
        <dl class="admin-delivery-sent-proof__grid">
          <dt>{t("delivery-sent-proof-ids-comparison")}</dt>
          <dd>{idsComparisonText(idsMatch())}</dd>
        </dl>
        <label for="delivery-sent-sender-name">{t("delivery-sent-proof-sender-name")}</label>
        <input
          id="delivery-sent-sender-name"
          name="sender_name"
          type="text"
          required
          disabled={disabled()}
          aria-invalid={
            fieldIsInvalid(localIssue(), "delivery-sent-sender-name") ? "true" : "false"
          }
          aria-describedby={fieldErrorId(localIssue(), "delivery-sent-sender-name") ?? undefined}
          value={current().senderName}
          onInput={(event) => {
            draft.update((next) => ({ ...next, senderName: event.currentTarget.value }));
            review.set(sentProofReviewClear(review.peek()));
          }}
        />
        {fieldError(localIssue(), "delivery-sent-sender-name")}
        <label for="delivery-sent-sender-uid">{t("delivery-sent-proof-sender-uid")}</label>
        <input
          id="delivery-sent-sender-uid"
          name="sender_uid"
          type="text"
          inputmode="numeric"
          required
          disabled={disabled()}
          aria-invalid={fieldIsInvalid(localIssue(), "delivery-sent-sender-uid") ? "true" : "false"}
          aria-describedby={fieldErrorId(localIssue(), "delivery-sent-sender-uid") ?? undefined}
          value={current().senderUid}
          onInput={(event) => {
            draft.update((next) => ({ ...next, senderUid: event.currentTarget.value }));
            review.set(sentProofReviewClear(review.peek()));
          }}
        />
        {fieldError(localIssue(), "delivery-sent-sender-uid")}
        <label for="delivery-sent-minute">{t("delivery-sent-proof-sent-minute")}</label>
        <input
          id="delivery-sent-minute"
          name="sent_minute"
          type="datetime-local"
          required
          disabled={disabled()}
          aria-invalid={fieldIsInvalid(localIssue(), "delivery-sent-minute") ? "true" : "false"}
          aria-describedby={fieldErrorId(localIssue(), "delivery-sent-minute") ?? undefined}
          value={current().sentMinute}
          onInput={(event) => {
            const next = draft.peek();
            const value = event.currentTarget.value;
            if (next.sentMinute === value) {
              return;
            }
            draft.set({ ...next, sentMinute: value, sentMinuteEdited: true });
            review.set(sentProofReviewClear(review.peek()));
          }}
        />
        {fieldError(localIssue(), "delivery-sent-minute")}
        <label for="delivery-sent-remaining-credits">
          {t("delivery-sent-proof-remaining-credits")}
        </label>
        <input
          id="delivery-sent-remaining-credits"
          name="remaining_credits"
          type="number"
          min="0"
          required
          disabled={disabled()}
          aria-invalid={
            fieldIsInvalid(localIssue(), "delivery-sent-remaining-credits") ? "true" : "false"
          }
          aria-describedby={
            fieldErrorId(localIssue(), "delivery-sent-remaining-credits") ?? undefined
          }
          value={current().remainingCredits}
          onInput={(event) => {
            draft.update((next) => ({
              ...next,
              remainingCredits: event.currentTarget.value,
            }));
            review.set(sentProofReviewClear(review.peek()));
          }}
        />
        {fieldError(localIssue(), "delivery-sent-remaining-credits")}
        <label for="delivery-sent-cod-pending">{t("delivery-sent-proof-cod-pending")}</label>
        <input
          id="delivery-sent-cod-pending"
          name="cod_pending"
          type="number"
          min="0"
          required
          disabled={disabled()}
          aria-invalid={
            fieldIsInvalid(localIssue(), "delivery-sent-cod-pending") ? "true" : "false"
          }
          aria-describedby={fieldErrorId(localIssue(), "delivery-sent-cod-pending") ?? undefined}
          value={current().codPending}
          onInput={(event) => {
            draft.update((next) => ({ ...next, codPending: event.currentTarget.value }));
            review.set(sentProofReviewClear(review.peek()));
          }}
        />
        {fieldError(localIssue(), "delivery-sent-cod-pending")}
        <label class="admin-delivery-sent-proof__check" for="delivery-sent-collected">
          <input
            id="delivery-sent-collected"
            type="checkbox"
            name="collected"
            disabled={disabled()}
            checked={current().collected}
            onChange={(event) => {
              draft.update((next) => ({
                ...next,
                collected: event.currentTarget.checked,
              }));
              review.set(sentProofReviewClear(review.peek()));
            }}
          />
          {t("delivery-sent-proof-collected")}
        </label>
        <Show when={current().collected}>
          <label for="delivery-sent-receipt">{t("delivery-sent-proof-receipt")}</label>
          <textarea
            id="delivery-sent-receipt"
            name="receipt"
            rows="3"
            disabled={disabled()}
            aria-invalid={fieldIsInvalid(localIssue(), "delivery-sent-receipt") ? "true" : "false"}
            aria-describedby={fieldErrorId(localIssue(), "delivery-sent-receipt") ?? undefined}
            value={current().receipt}
            onInput={(event) => {
              draft.update((next) => ({ ...next, receipt: event.currentTarget.value }));
              review.set(sentProofReviewClear(review.peek()));
            }}
          />
          {fieldError(localIssue(), "delivery-sent-receipt")}
        </Show>
        <label for="delivery-sent-reason">{t("delivery-sent-proof-reason")}</label>
        <textarea
          id="delivery-sent-reason"
          name="reason"
          rows="3"
          required
          disabled={disabled()}
          aria-invalid={reasonError() === null ? "false" : "true"}
          aria-describedby={reasonError() === null ? undefined : "delivery-sent-reason-error"}
          value={current().reason}
          onInput={(event) => {
            draft.update((next) => ({ ...next, reason: event.currentTarget.value }));
          }}
        />
        <Show when={reasonError()}>
          {(error) => (
            <p id="delivery-sent-reason-error" class="action-feedback action-feedback--error">
              {error()}
            </p>
          )}
        </Show>
        <p class="admin-panel__note">{t("delivery-sent-proof-note")}</p>
        <div class="admin-shell__dialog-actions">
          <button
            type="button"
            disabled={disabled() || reviewState().pending || missingIds()}
            onClick={() => triggerPreview()}
          >
            {previewLabel()}
          </button>
          <button type="button" disabled={props.busy} onClick={() => props.onCancel()}>
            {t("delivery-cancel")}
          </button>
          <button type="submit" class="admin-btn-primary" disabled={disabled() || !canConfirm()}>
            {confirmLabel()}
          </button>
        </div>
        <PreviewPanel reviewState={reviewState()} busy={disabled()} review={review} />
      </form>
    </Dialog>
  );
}

function equipmentIdsText(ids: readonly string[] | null): string {
  if (ids === null) {
    return t("delivery-sent-proof-ids-unrecorded");
  }
  if (ids.length === 0) {
    return t("delivery-sent-proof-ids-empty");
  }
  return ids.join(", ");
}

function idsComparisonText(matches: boolean | null): string {
  if (matches === null) {
    return t("delivery-sent-proof-ids-unrecorded");
  }
  return matches ? t("delivery-sent-proof-ids-match") : t("delivery-sent-proof-ids-mismatch");
}

function fieldIsInvalid(issue: SentProofIssue | null, field: string): boolean {
  return issue !== null && sentProofIssueFieldId(issue) === field;
}

function fieldErrorId(issue: SentProofIssue | null, field: string): string | null {
  return fieldIsInvalid(issue, field) ? `${field}-error` : null;
}

function fieldError(issue: SentProofIssue | null, field: string): JSX.Element {
  const current = issue !== null && sentProofIssueFieldId(issue) === field ? issue : null;
  return (
    <Show when={current !== null}>
      <p id={`${field}-error`} class="action-feedback action-feedback--error">
        {current === null ? "" : sentProofIssueMessage(current)}
      </p>
    </Show>
  );
}

function sameIds(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}
