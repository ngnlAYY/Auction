// 暂停回执独立于邮箱读取资源；错误后的重试只发送 GET。

import { expect, test } from "bun:test";
import { DELIVERY_RENDER, settle, withDeliveryDom } from "../deliveries/delivery-dom-fixture.test";
import {
  bodyOf,
  businessRequests,
  jsonReply,
  unavailableReply,
  withHttp,
} from "../deliveries/delivery-http-fixture.test";
import { firstOf } from "../deliveries/sent-proof/sent-proof-fixture";
import { pausedPollingWire, polling } from "./polling-fixture";

const READ_POLLING = "/api/v1/admin/mail?mailbox_key=mail-a&include_diagnostics=true";
const READ_INTAKE = "/api/v1/admin/mail/intake?page=1&page_size=10";
const WRITE = "/api/v1/admin/mail/polling";
const RECEIPT = "邮件轮询状态已更新。";
const UNAVAILABLE = "所需服务暂时不可用。请稍后再试。";

test("mail_poll_receipt_survives_the_failed_mailbox_read", async () => {
  const { MailPageFixture } = await DELIVERY_RENDER.mailPolling();
  const reads = [
    Promise.withResolvers<Response>(),
    Promise.withResolvers<Response>(),
    Promise.withResolvers<Response>(),
  ];
  const write = Promise.withResolvers<Response>();
  let readCount = 0;
  const state = polling("mail-a");
  const diagnostic = {
    completed_external_mail_id: null,
    cursor_status: "empty",
    observed_at: "2026-11-18T00:00:00Z",
    progress: null,
    worker_condition: "unavailable",
    worker_observed_at: null,
  };
  const paused = pausedPollingWire();
  await withHttp(
    (request) => {
      if (request.path === READ_POLLING) {
        const reply = reads[readCount];
        if (reply === undefined) throw new Error("额外的邮箱读取");
        readCount += 1;
        return reply.promise;
      }
      if (request.path === READ_INTAKE) {
        return jsonReply({
          code: "ok",
          message: "",
          data: { intakes: [], pagination: { page: 1, page_size: 10, total: 0 } },
        });
      }
      if (request.path === WRITE) return write.promise;
      throw new Error(`未预期的请求 ${request.path}`);
    },
    async (requests) =>
      withDeliveryDom("https://auction.test/mail?page_size=10", async (dom, mount) => {
        const [initialRead, failedRead, retryRead] = reads;
        if (initialRead === undefined || failedRead === undefined || retryRead === undefined)
          throw new Error("缺少邮箱读取回执");
        mount(MailPageFixture);
        await settle();
        dom.byId("mailbox-key").input("mail-a");
        dom
          .need((node) => node.tag === "form" && node.getAttribute("class") === "admin-mail-search")
          .submit();
        await settle();
        expect(businessRequests(requests)).toEqual([`GET ${READ_INTAKE}`, `GET ${READ_POLLING}`]);
        initialRead.resolve(
          jsonReply({
            code: "ok",
            message: "邮件轮询状态已加载",
            data: { polling: { ...state, diagnostics: diagnostic } },
          }),
        );
        await settle();
        dom.button("暂停轮询").click();
        await settle();
        expect(businessRequests(requests)).toEqual([
          `GET ${READ_INTAKE}`,
          `GET ${READ_POLLING}`,
          `PUT ${WRITE}`,
        ]);
        write.resolve(
          jsonReply({ code: "ok", message: "邮件轮询状态已更新", data: { polling: paused } }),
        );
        await settle();
        expect(dom.hasExactText(RECEIPT)).toBe(true);
        expect(businessRequests(requests)).toEqual([
          `GET ${READ_INTAKE}`,
          `GET ${READ_POLLING}`,
          `PUT ${WRITE}`,
          `GET ${READ_POLLING}`,
        ]);

        failedRead.resolve(unavailableReply());
        await settle();
        expect(dom.findButton("暂停轮询")).toBeNull();
        expect(dom.hasExactText(UNAVAILABLE)).toBe(true);
        expect(dom.hasExactText(RECEIPT)).toBe(true);
        expect(dom.findButton("关闭回执")).not.toBeNull();
        dom.button("重试").click();
        await settle();
        expect(businessRequests(requests)).toEqual([
          `GET ${READ_INTAKE}`,
          `GET ${READ_POLLING}`,
          `PUT ${WRITE}`,
          `GET ${READ_POLLING}`,
          `GET ${READ_POLLING}`,
        ]);
        retryRead.resolve(
          jsonReply({
            code: "ok",
            message: "邮件轮询状态已加载",
            data: { polling: { ...paused, diagnostics: diagnostic } },
          }),
        );
        await settle();
        expect(dom.findButton("恢复轮询")).not.toBeNull();
        expect(dom.hasExactText(RECEIPT)).toBe(true);
      }),
  );
});

test("mail_trigger_only_delivers_the_matching_accepted_count", async () => {
  const { MailPageFixture } = await DELIVERY_RENDER.mailPolling();
  const trigger = "/api/v1/admin/mail/polls:trigger";
  for (const mismatch of ["id", "count", "status", null]) {
    let submissions = 0;
    await withHttp(
      (request) => {
        if (request.path === READ_INTAKE) {
          return jsonReply({
            code: "ok",
            message: "邮件列表",
            data: { intakes: [], pagination: { page: 1, page_size: 10, total: 0 } },
          });
        }
        if (request.path !== trigger) throw new Error(`未预期的请求 ${request.path}`);
        const defect = submissions++ === 0 ? mismatch : null;
        return jsonReply(
          {
            code: "accepted",
            message: "已接受轮询",
            data: {
              command_id:
                defect === "id"
                  ? "00000000-0000-4000-8000-000000000099"
                  : bodyOf(request).command_id,
              pollers: defect === "count" ? 0 : 3,
            },
          },
          defect === "status" ? 200 : 202,
        );
      },
      (requests) =>
        withDeliveryDom("https://auction.test/mails?page_size=10", async (dom, mount) => {
          const accepted = "请求已接受，将唤醒 3 个轮询器。";
          mount(MailPageFixture);
          await settle();
          dom.button("唤醒全部邮件轮询器").click();
          await settle();
          expect(dom.hasExactText(accepted)).toBe(mismatch === null);
          expect(dom.findButton("原样重试") !== null).toBe(mismatch !== null);
          if (mismatch !== null) {
            expect(dom.button("唤醒全部邮件轮询器").disabled).toBe(true);
            dom.button("原样重试").click();
            await settle();
            expect(dom.hasExactText(accepted)).toBe(true);
            expect(dom.findButton("原样重试")).toBeNull();
          }
          expect(dom.findButton("关闭回执")).toBeNull();
          const writes = requests.filter((request) => request.path === trigger);
          expect(writes).toHaveLength(mismatch === null ? 1 : 2);
          expect(writes[0]?.method).toBe("POST");
          expect(bodyOf(firstOf(writes))).toEqual({ command_id: expect.any(String) });
          if (mismatch !== null) expect(writes[1]).toEqual(writes[0]);
        }),
    );
  }
});
