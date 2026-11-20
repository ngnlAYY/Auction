// 从真实预览入口核对本地拒绝、重复时刻和迟到的 fence 回执。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { ok } from "@auction/web-shared/result";
import { createComponent, createRoot } from "solid-js";
import { registerAdminSessionHost } from "../../../api/session";
import type { LocalTimeError } from "../../../api/validation";
import {
  DELIVERY_RENDER,
  type DeliveryDocument,
  settle,
  withDeliveryDom,
  withTestTimezone,
} from "../delivery-dom-fixture.test";
import { previewData } from "../delivery-http-fixture.test";
import { runPreview } from "./preview";
import { deferredPreview, stalePreview, withPreviewApi } from "./preview-api-fixture.test";
import { detail, evidence, filledDraft, firstOf } from "./sent-proof-fixture";
import { sentProofReviewDefault } from "./state";

const ID = "01010101-0101-0101-0101-010101010101";
const A = "mailbox_sweep:7000001";
const B = "mailbox_sweep:7000002";

function twoObservations() {
  const value = detail();
  return {
    ...value,
    evidence: [...value.evidence, { ...evidence(), reference: B, external_mail_id: "7000002" }],
  };
}

function fillSender(dom: DeliveryDocument): void {
  dom.byId("delivery-sent-sender-name").input("Sample");
  dom.byId("delivery-sent-sender-uid").input("12345");
  dom.byId("delivery-sent-reason").input("verified in Sent");
}

test("invalid_field_leaves_preview_idle_with_a_local_issue", async () => {
  const session = registerAdminSessionHost({
    check() {},
    writesAllowed: () => true,
    confirmed: () => null,
  });
  try {
    await withPreviewApi(
      () => {
        throw new Error("本地校验失败不得发出预览请求");
      },
      async (requests) => {
        createRoot((dispose) => {
          try {
            const value = detail();
            const draft = createCell({ ...filledDraft(value), senderUid: "" });
            const review = createCell(sentProofReviewDefault());
            runPreview(
              session.host,
              createTaskScope(),
              value,
              false,
              draft,
              review,
              () => {},
              () => {},
            );
            expect(review.peek().pending).toBe(false);
            expect(review.peek().outcome).toEqual({
              ok: false,
              error: { kind: "local", issue: { kind: "sender-uid" } },
            });
          } finally {
            dispose();
          }
        });
        await settle();
        expect(requests).toEqual([]);
      },
    );
  } finally {
    session.release();
  }
});

test("repeated_wall_minute_keeps_each_prefilled_utc_in_the_preview_request", async () => {
  const { SentProofDialog } = await DELIVERY_RENDER.sentProof();
  await withTestTimezone("America/Los_Angeles", async () => {
    for (const utc of ["2026-11-01T08:30:00Z", "2026-11-01T09:30:00Z"]) {
      const value = twoObservations();
      firstOf(value.evidence).sent_at = utc;
      await withPreviewApi(
        () => ok(previewData(ID, 3, "7000001")),
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
            expect(dom.byId("delivery-sent-minute").value).toBe("2026-11-01T01:30");
            fillSender(dom);
            dom.button("预览准入").click();
            await settle();
            expect(requests).toHaveLength(1);
            const [, id, reference, evidence] = firstOf(requests);
            expect(id).toBe(ID);
            expect(reference).toBe(A);
            expect(evidence.sent_minute).toBe(utc);
          }),
      );
    }
  });
});

test("edited_dst_gap_and_fold_render_time_field_errors_without_a_preview_request", async () => {
  const { SentProofDialog } = await DELIVERY_RENDER.sentProof();
  const validation = await import("./validation");
  const cases: readonly { wall: string; error: LocalTimeError; text: string }[] = [
    {
      wall: "2026-03-08T02:30",
      error: "nonexistent",
      text: "该时刻在本地时区不存在，请另选时间。",
    },
    {
      wall: "2026-11-01T01:30",
      error: "ambiguous",
      text: "该时刻在本地时区出现两次，请改选重复时段外的相邻时间。",
    },
  ];
  await withTestTimezone("America/Los_Angeles", async () => {
    for (const { wall, error, text } of cases) {
      await withPreviewApi(
        () => {
          throw new Error("无效的本地时刻不得发出预览请求");
        },
        async (requests) =>
          withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
            mount(() =>
              createComponent(SentProofDialog, {
                detail: twoObservations(),
                busy: false,
                onSubmit() {},
                onCancel() {},
                onStale() {},
              }),
            );
            await settle();
            fillSender(dom);
            const time = dom.byId("delivery-sent-minute");
            time.input(wall);
            expect(time.value).toBe(wall);
            const build = spyOn(validation, "buildEvidence");
            try {
              dom.button("预览准入").click();
              await settle();
              expect(build.mock.calls).toHaveLength(1);
              expect(firstOf(build.mock.calls)[1]).toMatchObject({
                sentMinute: wall,
                sentMinuteEdited: true,
              });
              expect(firstOf(build.mock.results)).toEqual({
                type: "return",
                value: { ok: false, error: { kind: "sent-minute", error } },
              });
            } finally {
              build.mockRestore();
            }
            expect(time.getAttribute("aria-describedby")).toBe("delivery-sent-minute-error");
            expect(time.getAttribute("aria-invalid")).toBe("true");
            expect(dom.byId("delivery-sent-minute-error").textContent).toBe(text);
            expect(requests).toEqual([]);
          }),
      );
    }
  });
});

test("only_the_current_stale_preview_notifies_detail_to_refresh", async () => {
  const { SentProofDialog } = await DELIVERY_RENDER.sentProof();
  const staleA = deferredPreview();
  const staleB = deferredPreview();
  await withPreviewApi(
    (_host, _id, reference) => {
      if (reference === A) return staleA.promise;
      if (reference === B) return staleB.promise;
      throw new Error("收到未知的预览证据");
    },
    async (requests) =>
      withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
        let refreshes = 0;
        mount(() =>
          createComponent(SentProofDialog, {
            detail: twoObservations(),
            busy: false,
            onSubmit() {},
            onCancel() {},
            onStale() {
              refreshes += 1;
            },
          }),
        );
        await settle();
        fillSender(dom);
        dom.button("预览准入").click();
        await settle();
        dom.byId("delivery-sent-evidence").change(B);
        dom.button("预览准入").click();
        await settle();
        expect(requests.map(([, id, reference]) => [id, reference])).toEqual([
          [ID, A],
          [ID, B],
        ]);
        staleA.resolve(stalePreview());
        await settle();
        expect(refreshes).toBe(0);
        expect(dom.button("正在预览…").disabled).toBe(true);
        staleB.resolve(stalePreview());
        await settle();
        expect(refreshes).toBe(1);
        const feedback = dom.need((node) => node.getAttribute("role") === "alert");
        expect(feedback.textContent).toContain("请重新选择证据");
      }),
  );
});
