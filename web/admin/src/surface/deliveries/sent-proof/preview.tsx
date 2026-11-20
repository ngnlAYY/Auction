// 预览与确认动作，以及服务端预览结果的展示。
// 预览只发送本地校验通过的安全投影且无副作用；确认沿用详情页命令宿主的稳定
// `command_id`，服务端仍会重新判定准入，不把预览当作成功证明。

import type {
  AdminDeliveryDetailResponse as DeliveryDetail,
  AdminDeliverySentProofPreviewResponse as DeliverySentProofPreview,
} from "@auction/contracts/dto/admin/delivery";
import type { Cell } from "@auction/web-shared/reactive/cell";
import type { TaskScope } from "@auction/web-shared/reactive/tasks";
import { err, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack } from "solid-js";

import { previewDeliverySentProof } from "../../../api/delivery";
import { type AdminApiFailure, adminApiFailureClient } from "../../../api/failure";
import type { AdminSessionHost } from "../../../api/session";
import { trimRustWhitespace } from "../../../api/session-time";
import { adminApiErrorMessage } from "../../presentation";
import {
  type AcceptedResponsibilities,
  type ReviewedSentProof,
  reviewedSentProofCanConfirm,
  reviewedSentProofSubmission,
  type SentProofDraft,
  type SentProofPreviewError,
  type SentProofRequest,
  type SentProofReview,
  type SentProofSubmission,
  sentProofIssueFieldId,
  sentProofReviewAcceptance,
  sentProofReviewBegin,
  sentProofReviewClear,
  sentProofReviewComplete,
  sentProofReviewRevision,
} from "./state";
import { buildEvidence, validReason } from "./validation";

/** 组装并发送无副作用预览；本地形状不合格时只记录问题码，不发请求。 */
export function runPreview(
  host: AdminSessionHost,
  tasks: TaskScope,
  detail: DeliveryDetail,
  busy: boolean,
  draft: Cell<SentProofDraft>,
  review: Cell<SentProofReview>,
  focusElement: (elementId: string) => void,
  onStale: () => void,
): void {
  if (busy || review.peek().pending) {
    return;
  }
  const current = draft.peek();
  const built = buildEvidence(detail, current);
  if (!built.ok) {
    const issue = built.error;
    const cleared = sentProofReviewClear(review.peek());
    const outcome: Result<ReviewedSentProof, SentProofPreviewError> = err({
      kind: "local",
      issue,
    });
    review.set({ ...cleared, outcome });
    focusElement(sentProofIssueFieldId(issue));
    return;
  }
  const request: SentProofRequest = {
    deliveryId: detail.delivery.id,
    fencingVersion: detail.delivery.fencing_version,
    evidenceReference: trimRustWhitespace(current.evidenceReference),
    evidence: built.value,
  };
  const pending = sentProofReviewBegin(review.peek());
  review.set(pending);
  const revision = pending.revision;
  tasks.spawn(async (context) => {
    const result = await requestPreview(host, request, context.signal);
    if (!context.alive()) {
      return;
    }
    const currentReview = review.peek();
    const completion = sentProofReviewComplete(currentReview, revision, request, result);
    if (completion.review !== currentReview) {
      review.set(completion.review);
    }
    if (completion.stale) {
      onStale();
    }
  });
}

async function requestPreview(
  host: AdminSessionHost,
  request: SentProofRequest,
  signal: AbortSignal,
): Promise<Result<DeliverySentProofPreview, AdminApiFailure>> {
  try {
    return await previewDeliverySentProof(
      host,
      request.deliveryId,
      request.evidenceReference,
      request.evidence,
      signal,
    );
  } catch {
    // 异常原文可能含秘密；预览失败统一按传输失败处理，不交给全局错误路径。
    return err(adminApiFailureClient("network_error"));
  }
}

/** 把一次性提交转交给命令宿主。 */
export function submitProof(
  deliveryId: string,
  fencingVersion: number,
  draft: Cell<SentProofDraft>,
  review: Cell<SentProofReview>,
  onSubmit: (submission: SentProofSubmission) => void,
): void {
  const reviewed = review.peek();
  if (reviewed.outcome === null || !reviewed.outcome.ok) {
    return;
  }
  const proof = reviewed.outcome.value;
  const current = draft.peek();
  if (
    !reviewedSentProofCanConfirm(proof, deliveryId, fencingVersion) ||
    !validReason(current.reason)
  ) {
    return;
  }
  const submission = reviewedSentProofSubmission(proof, current.reason);
  onSubmit(submission);
}

/** 服务端闭集 blocker 的译文；未知码原样显示，不猜测语义。 */
function blockerLabel(code: string): string {
  switch (code) {
    case "unsupported_delivery_scope":
      return t("delivery-sent-proof-blocker-scope");
    case "requires_unclaimed_write_unknown_or_admin_failure":
      return t("delivery-sent-proof-blocker-requires-unknown");
    case "fulfillment_identity_or_claim_conflict":
      return t("delivery-sent-proof-blocker-fulfillment");
    case "conflicting_recovery_disposition":
      return t("delivery-sent-proof-blocker-recovery");
    case "existing_binding_retirement_or_conflicting_attempt":
      return t("delivery-sent-proof-blocker-binding");
    case "requires_unique_ready_sender_funding_account":
      return t("delivery-sent-proof-blocker-sender-account");
    case "sender_player_name_mismatch":
      return t("delivery-sent-proof-blocker-sender-name");
    case "frozen_credential_retired_or_changed":
      return t("delivery-sent-proof-blocker-credential");
    case "sent_minute_outside_frozen_window":
      return t("delivery-sent-proof-blocker-minute-window");
    case "requires_exact_unknown_seller_reservation":
      return t("delivery-sent-proof-blocker-seller-reservation");
    case "frozen_identity_body_money_or_assets_mismatch":
      return t("delivery-sent-proof-blocker-frozen-identity");
    case "missing_observed_equipment_ids":
      return t("delivery-sent-proof-missing-observed-ids");
    case "observed_equipment_ids_mismatch":
      return t("delivery-sent-proof-ids-mismatch");
    default:
      return code;
  }
}

/** 预览结果面板：只展示服务端闭集判定，勾选项由服务端要求驱动。 */
export function PreviewPanel(props: {
  readonly reviewState: SentProofReview;
  readonly busy: boolean;
  readonly review: Cell<SentProofReview>;
}): JSX.Element {
  const outcome = () => props.reviewState.outcome;
  const apiFailure = () => {
    const current = outcome();
    return current !== null && !current.ok && current.error.kind === "api"
      ? current.error.failure
      : null;
  };
  const proof = () => {
    const current = outcome();
    return current?.ok ? current.value : null;
  };
  return (
    <Switch fallback={null}>
      <Match when={apiFailure()}>
        {(failure) => (
          <p class="action-feedback action-feedback--error" role="alert">
            {adminApiErrorMessage(failure().code)}
          </p>
        )}
      </Match>
      <Match when={proof()}>
        {(current) => {
          const preview = () => current().preview;
          const accepted = () => current().accepted;
          return (
            <section class="admin-delivery-sent-proof__preview" role="status">
              <Show
                when={preview().eligible}
                fallback={
                  <p class="action-feedback action-feedback--error">
                    {t("delivery-sent-proof-blocked")}
                  </p>
                }
              >
                <p>{t("delivery-sent-proof-eligible")}</p>
              </Show>
              <Show when={preview().corrects_admin_failure}>
                <p class="admin-panel__note">{t("delivery-sent-proof-corrects-failure")}</p>
              </Show>
              <Show when={preview().blockers.length > 0}>
                <ul class="admin-delivery-sent-proof__blockers">
                  <For each={preview().blockers} keyed={(blocker) => blocker}>
                    {(blocker) => <li>{blockerLabel(blocker())}</li>}
                  </For>
                </ul>
              </Show>
              <For
                each={[sentProofReviewRevision(props.reviewState)]}
                keyed={(revision) => revision}
              >
                {(revision) => {
                  // 责任区域按修订号重建；处理器只承认本区域的修订号，旧节点上的迟到点击
                  // 不会写进已经换代的预览。同一修订号内更新勾选继续复用节点。
                  const regionRevision = untrack(revision);
                  const setAcceptance = (
                    change: (value: AcceptedResponsibilities) => AcceptedResponsibilities,
                  ): void => {
                    const review = props.review.peek();
                    const next = sentProofReviewAcceptance(review, regionRevision, change);
                    if (next !== review) {
                      props.review.set(next);
                    }
                  };
                  return (
                    <>
                      <Show when={preview().requires_collected_asset_acceptance}>
                        <label
                          class="admin-delivery-sent-proof__check"
                          for="delivery-sent-accept-collected"
                        >
                          <input
                            id="delivery-sent-accept-collected"
                            type="checkbox"
                            name="accept_collected_assets"
                            disabled={props.busy}
                            checked={accepted().collectedAssets}
                            onChange={(event) => {
                              const checked = event.currentTarget.checked;
                              setAcceptance((value) => ({
                                ...value,
                                collectedAssets: checked,
                              }));
                            }}
                          />
                          {t("delivery-sent-proof-accept-collected")}
                        </label>
                      </Show>
                      <Show when={preview().prior_unbound_attempts === true}>
                        <label
                          class="admin-delivery-sent-proof__check"
                          for="delivery-sent-accept-prior-unbound"
                        >
                          <input
                            id="delivery-sent-accept-prior-unbound"
                            type="checkbox"
                            name="accept_prior_unbound_attempts"
                            disabled={props.busy}
                            checked={accepted().priorUnboundAttempts}
                            onChange={(event) => {
                              const checked = event.currentTarget.checked;
                              setAcceptance((value) => ({
                                ...value,
                                priorUnboundAttempts: checked,
                              }));
                            }}
                          />
                          {t("delivery-sent-proof-accept-attempts")}
                        </label>
                      </Show>
                      <Show when={preview().sent_before_current_attempt === true}>
                        <label
                          class="admin-delivery-sent-proof__check"
                          for="delivery-sent-accept-before-current"
                        >
                          <input
                            id="delivery-sent-accept-before-current"
                            type="checkbox"
                            name="accept_sent_before_current_attempt"
                            disabled={props.busy}
                            checked={accepted().sentBeforeCurrentAttempt}
                            onChange={(event) => {
                              const checked = event.currentTarget.checked;
                              setAcceptance((value) => ({
                                ...value,
                                sentBeforeCurrentAttempt: checked,
                              }));
                            }}
                          />
                          {t("delivery-sent-proof-accept-before-current")}
                        </label>
                      </Show>
                    </>
                  );
                }}
              </For>
            </section>
          );
        }}
      </Match>
    </Switch>
  );
}
