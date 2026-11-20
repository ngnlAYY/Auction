// 同一邮箱确认暂停后只刷新仍在显示的邮箱，已交付的回执随会话保留。

import { expect, spyOn, test } from "bun:test";
import { sendGet } from "@auction/web-shared/net/transport";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { createRoot } from "solid-js";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "../deliveries/delivery-dom-fixture.test";
import {
  bodyOf,
  businessRequests,
  jsonReply,
  withHttp,
} from "../deliveries/delivery-http-fixture.test";
import { firstOf } from "../deliveries/sent-proof/sent-proof-fixture";
import { pausedPollingWire } from "./polling-fixture";

const PUT = "/api/v1/admin/mail/polling";
const RECEIPT = "邮件轮询状态已更新。";

test("mailbox_switch_keeps_the_first_mailbox_receipt_and_does_not_reload_the_other_mailbox", async () => {
  const { PollingActionsFixture } = await DELIVERY_RENDER.mailPolling();
  for (const changeMailbox of [false, true]) {
    const reply = Promise.withResolvers<Response>();
    await withHttp(
      () => reply.promise,
      async (requests) =>
        withDeliveryDom("https://auction.test/mail", async (dom, mount) => {
          mount(PollingActionsFixture);
          await settle();
          dom.button("暂停轮询").click();
          await settle();
          if (changeMailbox) dom.button("切换邮箱").click();
          const writes = requests.filter((request) => request.path === PUT);
          expect(businessRequests(requests)).toEqual([`PUT ${PUT}`]);
          expect(bodyOf(firstOf(writes))).toMatchObject({
            mailbox_key: "mail-a",
            expected_state_version: 1,
            paused: true,
          });
          reply.resolve(
            jsonReply({
              code: "ok",
              message: "邮件轮询状态已更新",
              data: { polling: pausedPollingWire() },
            }),
          );
          await settle();
          expect(dom.findButton("关闭回执")).not.toBeNull();
          expect(dom.hasExactText(RECEIPT)).toBe(true);
          const generation = dom.byId("test-poll-generation");
          if (changeMailbox) {
            expect(generation.textContent).toBe("轮询世代 0");
            expect(dom.button("暂停轮询").disabled).toBe(false);
          } else {
            expect(generation.textContent).toBe("轮询世代 1");
            const pause = dom.button("暂停轮询");
            expect(pause.disabled).toBe(true);
            pause.dispatchClick();
            await settle();
            expect(generation.textContent).toBe("轮询世代 1");
            expect(dom.hasExactText(RECEIPT)).toBe(true);
          }
          expect(businessRequests(requests)).toEqual([`PUT ${PUT}`]);
        }),
    );
  }
});

test("断言失败卸载时取消迟到的回执并在恢复 fetch 前清除传输计时器", async () => {
  const timer = await import("@auction/web-shared/platform/timer");
  const oldFetch = globalThis.fetch;
  const reply = Promise.withResolvers<Response>();
  const interrupted = new Error("模拟回执前的断言失败");
  const replies: Awaited<ReturnType<typeof sendGet>>[] = [];
  const restoredOnCancel: boolean[] = [];
  const createTimeout = timer.createTimeoutFuture;
  const timeout = spyOn(timer, "createTimeoutFuture").mockImplementation((milliseconds) => {
    const scheduled = createTimeout(milliseconds);
    return {
      promise: scheduled.promise,
      cancel() {
        restoredOnCancel.push(globalThis.fetch === oldFetch);
        scheduled.cancel();
      },
    };
  });
  try {
    await expect(
      withHttp(
        () => reply.promise,
        async (requests) => {
          const mounted: { dispose?: () => void } = {};
          try {
            createRoot((release) => {
              mounted.dispose = release;
              createTaskScope().spawn(async (context) => {
                replies.push(await sendGet(PUT, context.signal));
              });
            });
            expect(businessRequests(requests)).toEqual([`GET ${PUT}`]);
            throw interrupted;
          } finally {
            mounted.dispose?.();
            await settle();
            // 请求须响应组件卸载；此时夹具的兜底取消尚未生效。
            expect(replies).toEqual([{ ok: false, error: { kind: "network", status: null } }]);
          }
        },
      ),
    ).rejects.toBe(interrupted);
    expect(restoredOnCancel).toEqual([false]);
    expect(globalThis.fetch).toBe(oldFetch);
  } finally {
    timeout.mockRestore();
    reply.resolve(jsonReply({}));
  }
});
