// 对账选项、证据播报与二次确认的取消焦点。

import { expect, test } from "bun:test";
import type {
  AdminDeliveryActionAvailability,
  AdminDeliveryActionsResponse,
} from "@auction/contracts/dto/admin/delivery";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createComponent } from "solid-js";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "./delivery-dom-fixture.test";
import {
  businessRequests,
  DETAIL_GET,
  detailFixture,
  detailFixtureWire,
  jsonReply,
  withHttp,
} from "./delivery-http-fixture.test";
import { selectedReconcileAction } from "./dialog-policy";
import { type DeliveryDialogKind, deliveryReconcileDraftForDelivery } from "./model";

test("advancing_confirmation_focuses_the_existing_cancel_button", async () => {
  const { DeliveryDialog } = await DELIVERY_RENDER.dialog();
  const payload = detailFixture();
  await withHttp(
    () => jsonReply(detailFixtureWire()),
    async (requests) =>
      withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
        const kind = createCell<DeliveryDialogKind>("reconcile");
        const delivery = payload.data.detail.delivery;
        mount(() =>
          createComponent(DeliveryDialog, {
            delivery,
            get kind() {
              return kind();
            },
            busy: false,
            draft: deliveryReconcileDraftForDelivery(delivery),
            onDraftChange: () => {},
            onConfirm: () => {},
            onCancel: () => {},
          }),
        );
        await settle();
        const cancel = dom.need(
          (node) =>
            node.tag === "button" &&
            node.textContent === "取消" &&
            node.getAttribute("autofocus") === null,
        );
        expect(cancel.focusCount).toBe(0);

        kind.set("reconcile_retry");
        await settle();
        expect(dom.need((node) => node === cancel)).toBe(cancel);
        expect(dom.activeElement).toBe(cancel);
        expect(cancel.focusCount).toBe(1);
        expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`]);
      }),
  );
});

test("evidence_status_announces_loading_and_empty_in_the_same_region", async () => {
  const { DeliveryEvidenceStatus } = await DELIVERY_RENDER.dialog();
  await withDeliveryDom("https://auction.test/deliveries", async (dom, mount) => {
    const message = createCell("正在读取证据");
    mount(() =>
      createComponent(DeliveryEvidenceStatus, {
        get message() {
          return message();
        },
      }),
    );
    const region = () => dom.need((node) => node.getAttribute("role") === "status");
    const initial = region();
    expect(initial.textContent).toBe("");
    await settle();
    expect(region()).toBe(initial);
    expect(initial.textContent).toBe("正在读取证据");
    message.set("没有证据");
    await settle();
    expect(region()).toBe(initial);
    expect(initial.textContent).toBe("没有证据");
  });
});

test("fresh_reconcile_draft_has_no_default_action", () => {
  const { data } = detailFixture("blocked-registration");
  expect(deliveryReconcileDraftForDelivery(data.detail.delivery).action).toBe("");
});

test("reconcile_requires_an_explicit_server_eligible_choice", () => {
  const blocked: AdminDeliveryActionAvailability = { state: "blocked", reason: "sent_sighted" };
  const actions: AdminDeliveryActionsResponse = {
    release: blocked,
    retry_failed: blocked,
    reconcile_retry: { state: "eligible" },
    reconcile_succeeded: blocked,
    reconcile_failed: blocked,
    acknowledge: blocked,
  };
  expect(selectedReconcileAction("", actions)).toBeNull();
  expect(selectedReconcileAction("retry", actions)).toBe("reconcile_retry");
  expect(selectedReconcileAction("succeeded", actions)).toBeNull();
  expect(selectedReconcileAction("retry", { ...actions, reconcile_retry: blocked })).toBeNull();
});
