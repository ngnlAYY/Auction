// 审核修订号隔离迟到的预览，并把确认限定在已审核的字段与勾选项。

import { expect, spyOn, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { createComponent } from "solid-js";
import { adminApiFailureClient } from "../../../api/failure";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "../delivery-dom-fixture.test";
import { selectableEvidence } from "./evidence";
import { deferredPreview, withPreviewApi } from "./preview-api-fixture.test";
import { detail, evidence, filledDraft, firstOf, requestAndPreview } from "./sent-proof-fixture";
import {
  type ReviewedSentProof,
  reviewedSentProofCanConfirm,
  reviewedSentProofSubmission,
  type SentProofReview,
  sentProofDraftWithEvidence,
  sentProofReviewAcceptance,
  sentProofReviewBegin,
  sentProofReviewClear,
  sentProofReviewComplete,
  sentProofReviewDefault,
} from "./state";

function proofOf(review: SentProofReview): ReviewedSentProof {
  if (review.outcome === null || !review.outcome.ok) throw new Error("预览没有生效");
  return review.outcome.value;
}

function acceptCollectedAssets(review: SentProofReview, revision: bigint): SentProofReview {
  return sentProofReviewAcceptance(review, revision, (accepted) => ({
    ...accepted,
    collectedAssets: true,
  }));
}

test("switching_a_to_b_discards_late_a_without_clearing_b_loading_or_preview", async () => {
  const a = requestAndPreview("7000001", true, false);
  const b = requestAndPreview("7000002", false, true);
  const aReply = deferredPreview();
  const bReply = deferredPreview();
  const secondAReply = deferredPreview();
  const replies = [aReply.promise, bReply.promise, secondAReply.promise];
  const { SentProofDialog } = await DELIVERY_RENDER.sentProof();
  const preview = await DELIVERY_RENDER.sentPreview();
  const state = await import("./state");
  const initial = detail();
  const value = {
    ...initial,
    evidence: [
      ...initial.evidence,
      { ...evidence(), reference: "mailbox_sweep:7000002", external_mail_id: "7000002" },
    ],
  };
  const completion = spyOn(state, "sentProofReviewComplete");
  try {
    const runPreview = spyOn(preview, "runPreview");
    try {
      await withPreviewApi(
        () => {
          const reply = replies.shift();
          if (reply === undefined) throw new Error("额外的预览请求");
          return reply;
        },
        async (requests) =>
          withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
            mount(() =>
              createComponent(SentProofDialog, {
                detail: value,
                busy: false,
                onSubmit() {},
                onCancel() {},
                onStale() {},
              }),
            );
            await settle();
            dom.byId("delivery-sent-sender-name").input("Sample");
            dom.byId("delivery-sent-sender-uid").input("12345");
            dom.byId("delivery-sent-reason").input("verified in Sent");
            const selector = dom.byId("delivery-sent-evidence");

            dom.button("预览准入").click();
            await settle();
            // 从真实预览入口拿到此对话框持有的信号，可在选择事件后直接读回。
            const [, , , , draft, review] = firstOf(runPreview.mock.calls);
            const firstARevision = review.peek().revision;
            selector.change("mailbox_sweep:7000002");
            expect(selector.value).toBe("mailbox_sweep:7000002");
            expect(draft.peek().evidenceReference).toBe("mailbox_sweep:7000002");
            expect(review.peek().pending).toBe(false);
            expect(dom.button("预览准入").disabled).toBe(false);
            dom.button("预览准入").click();
            await settle();
            const bRevision = review.peek().revision;
            aReply.resolve(ok(a.preview));
            await settle();
            expect(dom.button("正在预览…").disabled).toBe(true);
            expect(completion.mock.calls.map(([, revision]) => revision)).toEqual([firstARevision]);
            expect(review.peek()).toMatchObject({
              revision: bRevision,
              pending: true,
              outcome: null,
            });
            expect(dom.hasExactText("准入通过。确认时仍会重新核验快照。")).toBe(false);
            bReply.resolve(ok(b.preview));
            await settle();
            expect(dom.button("预览准入").disabled).toBe(false);
            expect(completion.mock.calls.map(([, revision]) => revision)).toEqual([
              firstARevision,
              bRevision,
            ]);
            expect(review.peek()).toMatchObject({
              revision: bRevision,
              pending: false,
              outcome: {
                ok: true,
                value: {
                  request: {
                    deliveryId: "01010101-0101-0101-0101-010101010101",
                    fencingVersion: 3,
                    evidenceReference: "mailbox_sweep:7000002",
                    evidence: { external_mail_id: "7000002" },
                  },
                },
              },
            });
            expect(dom.hasExactText("准入通过。确认时仍会重新核验快照。")).toBe(true);

            selector.change("mailbox_sweep:7000001");
            expect(selector.value).toBe("mailbox_sweep:7000001");
            expect(draft.peek().evidenceReference).toBe("mailbox_sweep:7000001");
            const cleared = review.peek();
            expect(cleared).toMatchObject({ pending: false, outcome: null });
            expect(cleared.revision).not.toBe(bRevision);
            expect(acceptCollectedAssets(cleared, bRevision)).toBe(cleared);
            expect(dom.hasExactText("准入通过。确认时仍会重新核验快照。")).toBe(false);
            dom.button("预览准入").click();
            await settle();
            expect(dom.button("正在预览…").disabled).toBe(true);
            secondAReply.resolve(ok(a.preview));
            await settle();
            expect(dom.button("预览准入").disabled).toBe(false);
            expect(review.peek().pending).toBe(false);
            expect(proofOf(review.peek()).request.evidenceReference).toBe("mailbox_sweep:7000001");
            expect(dom.hasExactText("准入通过。确认时仍会重新核验快照。")).toBe(true);
            expect(requests.map(([, , reference]) => reference)).toEqual([
              "mailbox_sweep:7000001",
              "mailbox_sweep:7000002",
              "mailbox_sweep:7000001",
            ]);
          }),
      );
    } finally {
      runPreview.mockRestore();
    }
  } finally {
    completion.mockRestore();
  }

  // 每个 HTTP 请求只响应一次。Rust 对旧修订号的重复交付在同一个状态入口上补齐。
  const begunA = sentProofReviewBegin(sentProofReviewDefault());
  const aRevision = begunA.revision;
  const begunB = sentProofReviewBegin(sentProofReviewClear(begunA));
  const bRevision = begunB.revision;
  const afterLateA = sentProofReviewComplete(begunB, aRevision, a.request, ok(a.preview)).review;
  expect(afterLateA.pending).toBe(true);
  expect(afterLateA.outcome).toBeNull();
  const completeB = sentProofReviewComplete(afterLateA, bRevision, b.request, ok(b.preview)).review;
  expect(completeB.pending).toBe(false);
  expect(acceptCollectedAssets(completeB, aRevision)).toBe(completeB);
  expect(proofOf(acceptCollectedAssets(completeB, bRevision)).accepted.collectedAssets).toBe(true);
  const lateError = sentProofReviewComplete(
    completeB,
    aRevision,
    a.request,
    err(adminApiFailureClient("old_error")),
  ).review;
  expect(reviewedSentProofSubmission(proofOf(lateError), "B").evidence).toEqual(b.request.evidence);
  const cleared = sentProofReviewClear(lateError);
  expect(cleared.outcome).toBeNull();
  expect(acceptCollectedAssets(cleared, bRevision)).toBe(cleared);
  const secondA = sentProofReviewBegin(cleared);
  const secondARevision = secondA.revision;
  const afterRepeatedA = sentProofReviewComplete(
    secondA,
    aRevision,
    a.request,
    ok(a.preview),
  ).review;
  expect(afterRepeatedA).toMatchObject({
    revision: secondARevision,
    pending: true,
    outcome: null,
  });
  const accepted = sentProofReviewComplete(
    afterRepeatedA,
    secondARevision,
    a.request,
    ok(a.preview),
  ).review;
  expect(accepted.pending).toBe(false);
  expect(accepted.outcome?.ok).toBe(true);
});

test("confirmation_uses_reviewed_fields_and_requires_each_triggered_responsibility", () => {
  const { request, preview } = requestAndPreview("7000001", true, true);
  preview.requires_collected_asset_acceptance = true;
  const started = sentProofReviewBegin(sentProofReviewDefault());
  const revision = started.revision;
  const delivered = sentProofReviewComplete(started, revision, request, ok(preview)).review;
  const ready = (review: SentProofReview) =>
    reviewedSentProofCanConfirm(proofOf(review), request.deliveryId, request.fencingVersion);

  let review = sentProofReviewAcceptance(delivered, revision, (accepted) => ({
    ...accepted,
    collectedAssets: true,
    priorUnboundAttempts: true,
  }));
  expect(ready(review)).toBe(false);
  review = sentProofReviewAcceptance(review, revision, (accepted) => ({
    ...accepted,
    sentBeforeCurrentAttempt: true,
  }));
  expect(ready(review)).toBe(true);

  const proof = proofOf(review);
  const submission = reviewedSentProofSubmission(proof, "verified");
  expect(submission.evidenceReference).toBe(request.evidenceReference);
  expect(submission.evidence).toEqual(request.evidence);
  expect(submission.expectedFencingVersion).toBe(request.fencingVersion);
  expect(submission.acceptCollectedAssets && submission.acceptAttemptHistory).toBe(true);
  expect(reviewedSentProofCanConfirm(proof, request.deliveryId, request.fencingVersion + 1)).toBe(
    false,
  );
  const next = sentProofReviewBegin(sentProofReviewClear(review));
  expect(ready(sentProofReviewComplete(next, next.revision, request, ok(preview)).review)).toBe(
    false,
  );
});

test("preview_with_changed_fence_cannot_be_used_for_confirmation", () => {
  const { request, preview } = requestAndPreview("7000001", false, false);
  preview.expected_fencing_version += 1;
  const started = sentProofReviewBegin(sentProofReviewDefault());
  const { review } = sentProofReviewComplete(started, started.revision, request, ok(preview));
  expect(review.outcome).toMatchObject({
    ok: false,
    error: { kind: "api", failure: { code: "stale_delivery_state" } },
  });
});

test("draft_preserves_operator_fields_across_evidence_switch", () => {
  const value = detail();
  const next = sentProofDraftWithEvidence(filledDraft(value), firstOf(selectableEvidence(value)));
  expect(next.senderUid).toBe("12345");
  expect(next.reason).toBe("verified in Sent");
  expect(next.remainingCredits).toBe("300000");
});
