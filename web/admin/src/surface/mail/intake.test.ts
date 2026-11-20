// 邮件详情的历史恢复、投递归属与原样重试。

import { expect, spyOn, test } from "bun:test";
import { setAt } from "../../api/api-request-fixture";
import { parseFixture, readAdminFixture } from "../../api/delivery/delivery-fixture.test";
import {
  DELIVERY_RENDER,
  type DeliveryDocument,
  settle,
  withDeliveryDom,
} from "../deliveries/delivery-dom-fixture.test";
import {
  bodyOf,
  DELIVERY_ID,
  detailFixtureWire,
  type HttpRequest,
  jsonReply,
  staleDeliveryReply,
  withHttp,
} from "../deliveries/delivery-http-fixture.test";
import { firstOf } from "../deliveries/sent-proof/sent-proof-fixture";

const PATH = "/api/v1/admin/mail/intake";
const A = "00000000-0000-4000-8000-000000000022";
const B = "00000000-0000-4000-8000-000000000021";
const SECOND_DELIVERY = "00000000-0000-4000-8000-000000000031";
const SUCCESS = "已重新排队，等待后台执行；这不表示邮件已发送、退回或领取成功。";
const INVALID = "服务器返回了无效响应。请重新加载后再试。";
const SELECT = "admin-mail__detail-trigger";

function listReply(query: URLSearchParams): Response {
  const wire = parseFixture(readAdminFixture("mail-intake"));
  const page = Number(query.get("page"));
  const size = Number(query.get("page_size"));
  if (query.get("world_id") === "isekai" || page !== 1) {
    setAt(wire, "/data/intakes", []);
  }
  setAt(wire, "/data/pagination", { page, page_size: size, total: 11 });
  return jsonReply(wire);
}

function intakeReply(
  id: string,
  deliveryIds = [id === A ? DELIVERY_ID : SECOND_DELIVERY],
): Response {
  const wire = parseFixture(readAdminFixture("mail-intake-detail"));
  setAt(wire, "/data/id", id);
  setAt(wire, "/data/external_mail_id", id === A ? "9002" : "9001");
  setAt(
    wire,
    "/data/deliveries",
    deliveryIds.map((deliveryId) => ({
      id: deliveryId,
      operation: "send",
      recipient: "Sample",
      status: "manual_reconcile",
      fencing_version: 4,
      attempt_count: 1,
      failure_code: "write_unknown",
      reason_code: null,
      external_reference: null,
    })),
  );
  setAt(wire, "/data/metadata/delivery_ids", deliveryIds);
  return jsonReply(wire);
}

function deliveryReply(id: string): Response {
  const wire = detailFixtureWire();
  setAt(wire, "/data/detail/delivery/id", id);
  setAt(wire, "/data/actions/reconcile_retry", { state: "eligible" });
  return jsonReply(wire);
}

function selectIntake(dom: DeliveryDocument, mailId: string): void {
  dom.need((node) => node.getAttribute("class") === SELECT && node.textContent === mailId).click();
}

function openDelivery(dom: DeliveryDocument, id: string): void {
  const row = dom.need((node) => node.tag === "tr" && node.textContent.includes(id));
  const button = row.querySelector("button");
  if (button === null) throw new Error("缺少处置入口");
  button.click();
}

async function queueDelivery(dom: DeliveryDocument, id: string): Promise<void> {
  openDelivery(dom, id);
  await settle();
  const confirm = dom.button("确认");
  expect(confirm.disabled).toBe(false);
  confirm.click();
  await settle();
}

test("mail_retry_replaces_only_its_own_notice_and_reports_after_panel_unmount", async () => {
  const { MailPageFixture } = await DELIVERY_RENDER.mailPolling();
  const timer = await import("@auction/web-shared/platform/timer");
  const createTimeout = timer.createTimeoutFuture;
  let expire: (() => void) | null = null;
  const timeout = spyOn(timer, "createTimeoutFuture").mockImplementation((milliseconds) => {
    const scheduled = createTimeout(milliseconds);
    const forced = Promise.withResolvers<void>();
    expire = () => forced.resolve();
    return { promise: Promise.race([scheduled.promise, forced.promise]), cancel: scheduled.cancel };
  });
  let writesA = 0;
  try {
    await withHttp(
      (request) => {
        const url = new URL(request.path, "https://auction.test");
        if (url.pathname === PATH) return listReply(url.searchParams);
        if (url.pathname === `${PATH}/${A}`) return intakeReply(A);
        if (url.pathname === `${PATH}/${B}`) return intakeReply(B);
        if (request.method === "GET") {
          return deliveryReply(url.pathname.endsWith(DELIVERY_ID) ? DELIVERY_ID : SECOND_DELIVERY);
        }
        if (url.pathname === "/api/v1/browser-errors") return new Response(null, { status: 204 });
        if (url.pathname.endsWith(`/${SECOND_DELIVERY}/reconcile`)) return staleDeliveryReply();
        writesA += 1;
        if (writesA === 1) {
          if (expire === null) throw new Error("缺少写请求的超时计时器");
          expire();
          return new Promise<Response>(() => {});
        }
        const wire = parseFixture(readAdminFixture("delivery-release-command"));
        setAt(wire, "/data/delivery/id", writesA === 2 ? SECOND_DELIVERY : DELIVERY_ID);
        return jsonReply(wire);
      },
      (requests) =>
        withDeliveryDom("https://auction.test/mails?page_size=10", async (dom, mount) => {
          mount(MailPageFixture);
          await settle();
          selectIntake(dom, "9002");
          await settle();
          await queueDelivery(dom, DELIVERY_ID);
          expect(dom.findButton("原样重试")).not.toBeNull();
          expect(
            dom.hasExactText(
              `${DELIVERY_ID} · 请求超时，服务端结果可能尚未返回。表单内容已保留，请在当前页面再次提交；相同命令不会重复录入。`,
            ),
          ).toBe(true);

          selectIntake(dom, "9001");
          await settle();
          await queueDelivery(dom, SECOND_DELIVERY);
          const noticeB = `${SECOND_DELIVERY} · 投递状态或证据已变化，请核对最新详情后重试。提交 Sent 证明时请重新选择证据。`;
          expect(dom.hasExactText(noticeB)).toBe(true);
          expect(dom.findButton("关闭回执")).toBeNull();
          expect(dom.hasExactText(`${DELIVERY_ID} · ${INVALID}`)).toBe(false);

          dom.button("原样重试").click();
          await settle();
          expect(dom.hasExactText(noticeB)).toBe(true);
          expect(dom.findButton("关闭回执")).toBeNull();
          selectIntake(dom, "9002");
          await settle();
          expect(dom.hasExactText(`${DELIVERY_ID} · ${INVALID}`)).toBe(true);
          expect(dom.hasExactText(noticeB)).toBe(false);

          selectIntake(dom, "9001");
          await settle();
          expect(dom.hasExactText(noticeB)).toBe(true);
          dom.button("原样重试").click();
          await settle();
          expect(dom.hasExactText(noticeB)).toBe(true);
          expect(dom.hasExactText(SUCCESS)).toBe(true);
          expect(
            dom.root.querySelectorAll("button").filter((node) => node.textContent === "关闭回执"),
          ).toHaveLength(1);
          expect(dom.findButton("原样重试")).toBeNull();
          selectIntake(dom, "9002");
          await settle();
          expect(dom.hasExactText(`${DELIVERY_ID} · ${SUCCESS}`)).toBe(true);
          expect(dom.hasExactText(`${DELIVERY_ID} · ${INVALID}`)).toBe(false);
          dom.button("关闭详情").click();
          await settle();
          selectIntake(dom, "9002");
          await settle();
          expect(dom.hasExactText(`${DELIVERY_ID} · ${SUCCESS}`)).toBe(false);
          selectIntake(dom, "9001");
          await settle();
          expect(dom.hasExactText(noticeB)).toBe(true);

          const writes = requests.filter(
            ({ method, path }) => method === "POST" && path.endsWith("/reconcile"),
          );
          expect(writes.map(({ path }) => path)).toEqual([
            `/api/v1/admin/deliveries/${DELIVERY_ID}/reconcile`,
            `/api/v1/admin/deliveries/${SECOND_DELIVERY}/reconcile`,
            `/api/v1/admin/deliveries/${DELIVERY_ID}/reconcile`,
            `/api/v1/admin/deliveries/${DELIVERY_ID}/reconcile`,
          ]);
          const first = firstOf(writes);
          expect(bodyOf(first)).toEqual({
            command_id: expect.any(String),
            expected_fencing_version: 4,
            action: "retry",
          });
          expect(writes.slice(2)).toEqual([first, first]);
        }),
    );
  } finally {
    timeout.mockRestore();
  }
});

test("mail_filter_history_discards_late_detail_and_open_dialog", async () => {
  const { MailPageFixture } = await DELIVERY_RENDER.mailPolling();
  const delayed = Promise.withResolvers<Response>();
  let first = true;
  let retried = false;
  const reads = (requests: readonly HttpRequest[]) =>
    requests.filter(({ path }) => path === `${PATH}/${A}`).length;
  await withHttp(
    (request) => {
      const url = new URL(request.path, "https://auction.test");
      if (url.pathname === PATH) return listReply(url.searchParams);
      if (url.pathname === `${PATH}/${A}`) {
        if (first) {
          first = false;
          return delayed.promise;
        }
        return intakeReply(A);
      }
      if (url.pathname === `/api/v1/admin/deliveries/${DELIVERY_ID}`)
        return deliveryReply(DELIVERY_ID);
      if (url.pathname === `/api/v1/admin/deliveries/${DELIVERY_ID}/reconcile`) {
        if (!retried) {
          retried = true;
          return Promise.reject(new Error("模拟写入后断开"));
        }
        const wire = parseFixture(readAdminFixture("delivery-release-command"));
        setAt(wire, "/data/delivery/id", DELIVERY_ID);
        return jsonReply(wire);
      }
      throw new Error(`未预期的请求 ${request.path}`);
    },
    (requests) =>
      withDeliveryDom("https://auction.test/mails?page_size=10", async (dom, mount) => {
        mount(MailPageFixture);
        await settle();
        selectIntake(dom, "9002");
        await settle();
        expect(dom.hasExactText("正在加载收件详情……")).toBe(true);
        dom.byId("mail-world").change("isekai");
        await settle();
        delayed.resolve(intakeReply(A));
        await settle();
        expect(dom.hasExactText("正在加载收件详情……")).toBe(false);
        expect(dom.findButton("查看处置")).toBeNull();
        window.history.back();
        await settle();
        expect(dom.findButton("关闭详情")).toBeNull();
        expect(reads(requests)).toBe(1);

        selectIntake(dom, "9002");
        await settle();
        window.history.replaceState(window.history.state, "", "/mails?world=&page=1&page_size=10");
        window.dispatchEvent(new Event("popstate"));
        await settle();
        expect(dom.findButton("关闭详情")).not.toBeNull();
        expect(reads(requests)).toBe(2);
        window.history.replaceState(
          window.history.state,
          "",
          "/mails?page=2&page_size=10&sort=received&dir=desc",
        );
        window.dispatchEvent(new Event("popstate"));
        await settle();
        expect(dom.findButton("关闭详情")).not.toBeNull();
        expect(reads(requests)).toBe(2);
        expect(requests.some(({ path }) => path === `${PATH}?page=2&page_size=10`)).toBe(true);
        dom.button("查看处置").click();
        await settle();
        expect(dom.button("确认").disabled).toBe(false);
        window.history.forward();
        await settle();
        expect(dom.find((node) => node.id === "mail-delivery-action-dialog")).toBeNull();
        expect(dom.findButton("关闭详情")).toBeNull();
        window.history.back();
        await settle();
        expect(dom.findButton("查看处置")).toBeNull();
        expect(reads(requests)).toBe(2);
        expect(requests.filter(({ method }) => method !== "GET")).toEqual([]);

        // 筛选清除行与对话框，已发出的业务命令仍由会话宿主持有。
        window.history.replaceState(window.history.state, "", "/mails?page_size=10");
        window.dispatchEvent(new Event("popstate"));
        await settle();
        selectIntake(dom, "9002");
        await settle();
        await queueDelivery(dom, DELIVERY_ID);
        expect(dom.button("原样重试").disabled).toBe(false);
        dom.byId("mail-world").change("isekai");
        await settle();
        expect(dom.findButton("查看处置")).toBeNull();
        dom.button("原样重试").click();
        await settle();
        expect(dom.findButton("原样重试")).toBeNull();
        expect(dom.hasExactText(SUCCESS)).toBe(true);
        const writes = requests.filter(({ method }) => method === "POST");
        expect(writes).toHaveLength(2);
        expect(writes[1]).toEqual(writes[0]);
      }),
  );
});

test.each([
  ["kind", "bid"],
  ["status", "failed"],
])("mail_filter_history_discards_detail_after_changing_%s", async (filter, value) => {
  const { MailPageFixture } = await DELIVERY_RENDER.mailPolling();
  await withHttp(
    (request) => {
      const url = new URL(request.path, "https://auction.test");
      if (url.pathname === PATH) {
        if (!url.searchParams.has(filter)) return listReply(url.searchParams);
        return jsonReply({
          code: "ok",
          message: "邮件入站列表已加载",
          data: { intakes: [], pagination: { page: 1, page_size: 10, total: 0 } },
        });
      }
      if (url.pathname === `${PATH}/${A}`) return intakeReply(A);
      if (url.pathname === `/api/v1/admin/deliveries/${DELIVERY_ID}`)
        return deliveryReply(DELIVERY_ID);
      throw new Error(`未预期的请求 ${request.path}`);
    },
    (requests) =>
      withDeliveryDom("https://auction.test/mails?page_size=10", async (dom, mount) => {
        mount(MailPageFixture);
        await settle();
        selectIntake(dom, "9002");
        await settle();
        expect(dom.findButton("关闭详情")).not.toBeNull();
        openDelivery(dom, DELIVERY_ID);
        await settle();
        expect(dom.button("确认").disabled).toBe(false);

        dom.byId(`mail-${filter}`).change(value);
        await settle();
        expect(
          requests.some(
            ({ path }) => new URL(path, "https://auction.test").searchParams.get(filter) === value,
          ),
        ).toBe(true);
        expect(dom.findButton("关闭详情")).toBeNull();
        expect(dom.findButton("查看处置")).toBeNull();
        expect(dom.find((node) => node.id === "mail-delivery-action-dialog")).toBeNull();

        window.history.back();
        await settle();
        expect(
          dom.find((node) => node.getAttribute("class") === SELECT && node.textContent === "9002"),
        ).not.toBeNull();
        expect(dom.findButton("关闭详情")).toBeNull();
        expect(dom.findButton("查看处置")).toBeNull();
        expect(dom.find((node) => node.id === "mail-delivery-action-dialog")).toBeNull();
        expect(requests.filter(({ path }) => path === `${PATH}/${A}`)).toHaveLength(1);
        expect(requests.every(({ method }) => method === "GET")).toBe(true);
      }),
  );
});

test("mail_new_attempt_clears_its_notice_and_unmounted_page_leaves_receipt_to_the_shell", async () => {
  const { UnmountableMailPageFixture } = await DELIVERY_RENDER.mailPolling();
  const late = Promise.withResolvers<Response>();
  let writes = 0;
  await withHttp(
    (request) => {
      const url = new URL(request.path, "https://auction.test");
      if (url.pathname === PATH) return listReply(url.searchParams);
      if (url.pathname === `${PATH}/${A}`) return intakeReply(A, [DELIVERY_ID, SECOND_DELIVERY]);
      const id = url.pathname.includes(DELIVERY_ID) ? DELIVERY_ID : SECOND_DELIVERY;
      if (request.method === "GET") return deliveryReply(id);
      if (++writes === 3) return late.promise;
      const wire = parseFixture(readAdminFixture("delivery-release-command"));
      setAt(wire, "/data/delivery/id", id);
      return jsonReply(wire);
    },
    () =>
      withDeliveryDom("https://auction.test/mails?page_size=10", async (dom, mount) => {
        mount(UnmountableMailPageFixture);
        await settle();
        selectIntake(dom, "9002");
        await settle();
        await queueDelivery(dom, DELIVERY_ID);
        await queueDelivery(dom, SECOND_DELIVERY);
        const first = `${DELIVERY_ID} · ${SUCCESS}`;
        const second = `${SECOND_DELIVERY} · ${SUCCESS}`;
        expect(dom.hasExactText(first)).toBe(true);
        expect(dom.hasExactText(second)).toBe(true);
        openDelivery(dom, DELIVERY_ID);
        await settle();
        expect(dom.hasExactText(first)).toBe(false);
        expect(dom.hasExactText(second)).toBe(true);
        dom.button("确认").click();
        await settle();
        expect(writes).toBe(3);
        dom.button("切换邮件页").click();
        await settle();
        const wire = parseFixture(readAdminFixture("delivery-release-command"));
        setAt(wire, "/data/delivery/id", DELIVERY_ID);
        late.resolve(jsonReply(wire));
        await settle();
        expect(dom.hasExactText(SUCCESS)).toBe(true);
        expect(dom.hasExactText(first)).toBe(false);
        dom.button("切换邮件页").click();
        await settle();
        selectIntake(dom, "9002");
        await settle();
        expect(dom.hasExactText(first)).toBe(false);
        expect(dom.hasExactText(second)).toBe(false);
        expect(dom.hasExactText(SUCCESS)).toBe(true);
      }),
  );
});
