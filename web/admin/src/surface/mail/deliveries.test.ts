// 用关联投递行的可见处置入口核对服务端 fence 的重排判定。

import { expect, test } from "bun:test";
import type {
  AdminDeliveryActionAvailability,
  AdminDeliveryActionsResponse,
} from "@auction/contracts/dto/admin/delivery";
import { createComponent } from "solid-js";
import { setAt } from "../../api/api-request-fixture";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "../deliveries/delivery-dom-fixture.test";
import {
  bodyOf,
  businessRequests,
  DETAIL_GET,
  detailFixtureWire,
  jsonReply,
  unavailableReply,
  withHttp,
} from "../deliveries/delivery-http-fixture.test";

test("linked_mail_uses_the_fence_decision_for_retry", async () => {
  const { LinkedDeliveryFixture } = await DELIVERY_RENDER.mailDeliveries();
  const mismatch: AdminDeliveryActionAvailability = { state: "blocked", reason: "status_mismatch" };
  const actions: AdminDeliveryActionsResponse = {
    release: mismatch,
    retry_failed: mismatch,
    reconcile_retry: { state: "eligible" },
    reconcile_succeeded: mismatch,
    reconcile_failed: mismatch,
    acknowledge: mismatch,
  };
  const reconcilePath = "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/reconcile";
  for (const ownedByReturn of [false, true]) {
    const fixture = detailFixtureWire();
    setAt(
      fixture,
      "/data/actions",
      ownedByReturn
        ? {
            ...actions,
            reconcile_retry: { state: "blocked", reason: "settlement_return_action_owned" },
          }
        : actions,
    );
    await withHttp(
      (request) => {
        if (request.path === DETAIL_GET) return jsonReply(fixture);
        if (request.path === reconcilePath) return unavailableReply();
        throw new Error(`未预期的请求 ${request.path}`);
      },
      async (requests) =>
        withDeliveryDom("https://auction.test/mail", async (dom, mount) => {
          mount(() =>
            createComponent(LinkedDeliveryFixture, {
              delivery: {
                id: "0f8fad5b-d9cb-469f-a165-70867728950e",
                operation: "send",
                recipient: "Sample",
                status: "manual_reconcile",
                fencing_version: 4,
                attempt_count: 1,
                failure_code: "write_unknown",
                reason_code: null,
                external_reference: null,
              },
            }),
          );
          await settle();
          dom.button("查看处置").click();
          await settle();
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`]);
          if (ownedByReturn) {
            expect(dom.findButton("确认")).toBeNull();
            expect(dom.hasExactText("这项邮件操作由退件案件处理。")).toBe(true);
            expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`]);
          } else {
            const confirm = dom.button("确认");
            expect(confirm.disabled).toBe(false);
            confirm.click();
            await settle();
            expect(businessRequests(requests)).toEqual([
              `GET ${DETAIL_GET}`,
              `POST ${reconcilePath}`,
            ]);
            const command = requests.find((request) => request.path === reconcilePath);
            if (command === undefined) throw new Error("缺少重排请求");
            expect(bodyOf(command)).toMatchObject({ action: "retry", expected_fencing_version: 4 });
          }
        }),
    );
  }
});
