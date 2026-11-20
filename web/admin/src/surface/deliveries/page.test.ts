// 队列写入后的重读与较早未决命令的裁定。

import { expect, test } from "bun:test";
import { setAt } from "../../api/api-request-fixture";
import { parseFixture, readAdminFixture } from "../../api/delivery/delivery-fixture.test";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "./delivery-dom-fixture.test";
import {
  bodyOf,
  businessRequests,
  DELIVERY_ID,
  DETAIL_GET,
  detailFixtureWire,
  jsonReply,
  withHttp,
} from "./delivery-http-fixture.test";
import { firstOf } from "./sent-proof/sent-proof-fixture";

const LIST = "/api/v1/admin/deliveries?page=1&page_size=50&include=settlement_returns";
const RETRY = `/api/v1/admin/deliveries/${DELIVERY_ID}/retry`;

test("queue_rereads_on_stale_or_renamed_and_retains_an_earlier_unknown_retry", async () => {
  const { page } = await DELIVERY_RENDER.queue();
  for (const rejection of ["stale_delivery_state", "delivery_recipient_renamed"]) {
    let attempts = 0;
    const detail = detailFixtureWire();
    setAt(detail, "/data/detail/delivery/status", "failed");
    setAt(detail, "/data/detail/evidence", []);
    setAt(detail, "/data/actions/retry_failed", { state: "eligible" });
    const list = parseFixture(readAdminFixture("deliveries"));
    setAt(list, "/data/deliveries/0/id", DELIVERY_ID);
    await withHttp(
      (request) => {
        if (request.path === LIST) return jsonReply(list);
        if (request.path === DETAIL_GET) return jsonReply(detail);
        if (request.path !== RETRY) throw new Error(`未预期的请求 ${request.path}`);
        if (attempts++ === 0) return Promise.reject(new Error("模拟写入后断开"));
        return jsonReply(
          { code: rejection, message: "队列已变化", data: null, request_id: null },
          409,
        );
      },
      (requests) =>
        withDeliveryDom("https://auction.test/deliveries?page_size=50", async (dom, mount) => {
          mount(page);
          await settle();
          dom.button("查看处置").click();
          await settle();
          expect(dom.button("确认").disabled).toBe(false);
          dom.button("确认").click();
          await settle();
          const beforeRetry = [`GET ${LIST}`, `GET ${DETAIL_GET}`, `POST ${RETRY}`];
          expect(businessRequests(requests)).toEqual(beforeRetry);
          expect(dom.button("原样重试").disabled).toBe(false);
          dom.button("原样重试").click();
          await settle();
          expect(businessRequests(requests)).toEqual([
            ...beforeRetry,
            `POST ${RETRY}`,
            `GET ${LIST}`,
          ]);
          const writes = requests.filter(({ path }) => path === RETRY);
          expect(bodyOf(firstOf(writes))).toEqual({
            command_id: expect.any(String),
            expected_fencing_version: 4,
          });
          expect(writes[1]).toEqual(writes[0]);
          expect(dom.button("原样重试").disabled).toBe(false);
        }),
    );
  }
});
