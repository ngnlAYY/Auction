// 在实际投递详情页验证跨来源占用、失败重读回执与 stale fence。

import { expect, spyOn, test } from "bun:test";
import { useRouter } from "@auction/web-shared/router";
import { createComponent } from "solid-js";
import { setAt } from "../../../api/api-request-fixture";
import {
  DELIVERY_RENDER,
  type DeliveryDocument,
  settle,
  withDeliveryDom,
  withTestTimezone,
} from "../delivery-dom-fixture.test";
import {
  bodyOf,
  businessRequests,
  DELIVERY_ID,
  DETAIL_GET,
  detailFixture,
  detailFixtureWire,
  jsonReply,
  previewData,
  previewPath,
  previewReply,
  staleDeliveryReply,
  unavailableReply,
  withHttp,
} from "../delivery-http-fixture.test";
import { firstOf } from "../sent-proof/sent-proof-fixture";

const PAGE = `https://auction.test/deliveries/${DELIVERY_ID}`;
const CONFIRM_POST =
  "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/sent-proof/confirm";
const PREVIEW_POST = previewPath(DELIVERY_ID);
const SENT_RECEIPT = "人工 Sent 证明已提交。";
const ELIGIBLE = "准入通过。确认时仍会重新核验快照。";
const UNAVAILABLE = "所需服务暂时不可用。请稍后再试。";
const OPEN = "打开人工 Sent 证明";
const CONFIRM = "确认证明";

function acknowledgeFixture(): unknown {
  const wire = detailFixtureWire();
  setAt(wire, "/data/actions/acknowledge", { state: "eligible" });
  return wire;
}

async function openReviewedProof(dom: DeliveryDocument) {
  dom.button(OPEN).click();
  await settle();
  dom.byId("delivery-sent-sender-name").input("Sample");
  dom.byId("delivery-sent-sender-uid").input("12345");
  dom.byId("delivery-sent-minute").input("2026-11-02T01:14");
  dom.byId("delivery-sent-reason").input("已核对 Sent 实见");
  dom.button("预览准入").click();
  await settle();
  expect(dom.hasExactText(ELIGIBLE)).toBe(true);
  expect(dom.button(CONFIRM).disabled).toBe(false);
  return dom.need(
    (node) => node.tag === "form" && node.getAttribute("class") === "admin-delivery-sent-proof",
  );
}

test("another_sources_pending_delivery_blocks_detail_entries_and_the_open_dialog", async () => {
  const { DetailWithListCommand } = await DELIVERY_RENDER.detailWithList();
  const wire = acknowledgeFixture();
  await withTestTimezone("UTC", () =>
    withHttp(
      (request) => {
        if (request.path === DETAIL_GET) return jsonReply(wire);
        if (request.path === PREVIEW_POST)
          return previewReply(previewData(DELIVERY_ID, 4, "7000001"));
        throw new Error(`未预期的请求 ${request.path}`);
      },
      async (requests) =>
        withDeliveryDom(PAGE, async (dom, mount) => {
          mount(() =>
            createComponent(DetailWithListCommand, {
              id: DELIVERY_ID,
              onSubmitResult: (submitted) => expect(submitted).toBe(true),
            }),
          );
          await settle();
          dom
            .need((node) => node.tag === "textarea" && node.getAttribute("rows") === "3")
            .input("已核对");
          expect(dom.button("确认已处置").disabled).toBe(false);
          const form = await openReviewedProof(dom);
          dom.button("列表提交同一投递").click();
          await settle();
          expect(dom.hasExactText("这条投递还有未了结的操作，请先在未决区核对。")).toBe(true);
          expect(dom.hasExactText(ELIGIBLE)).toBe(true);
          expect(dom.button("确认已处置").disabled).toBe(true);
          expect(dom.button(CONFIRM).disabled).toBe(true);
          form.submit();
          await settle();
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`, `POST ${PREVIEW_POST}`]);
          expect(dom.button(OPEN).disabled).toBe(true);
          dom.button("取消").click();
          dom.button(OPEN).dispatchClick();
          await settle();
          expect(dom.findButton(CONFIRM)).toBeNull();
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`, `POST ${PREVIEW_POST}`]);
        }),
    ),
  );
});

test("sent_proof_receipt_survives_the_failed_delivery_read", async () => {
  const { detailPage } = await DELIVERY_RENDER.detail();
  const wire = acknowledgeFixture();
  const confirm = Promise.withResolvers<Response>();
  let reads = 0;
  const commandId = "01010101-0101-0101-0101-010101010101";
  const uuid = await import("@auction/web-shared/platform/uuid");
  const commandUuid = spyOn(uuid, "createUuidV4").mockReturnValue(commandId);
  try {
    await withTestTimezone("UTC", () =>
      withHttp(
        (request) => {
          if (request.path === DETAIL_GET) {
            reads += 1;
            return reads === 2 ? unavailableReply() : jsonReply(wire);
          }
          if (request.path === PREVIEW_POST)
            return previewReply(previewData(DELIVERY_ID, 4, "7000001"));
          if (request.path === CONFIRM_POST) return confirm.promise;
          throw new Error(`未预期的请求 ${request.path}`);
        },
        async (requests) =>
          withDeliveryDom(PAGE, async (dom, mount) => {
            mount(() => detailPage(DELIVERY_ID));
            await settle();
            expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`]);
            const form = await openReviewedProof(dom);
            form.submit();
            await settle();
            const beforeReply = [
              `GET ${DETAIL_GET}`,
              `POST ${PREVIEW_POST}`,
              `POST ${CONFIRM_POST}`,
            ];
            expect(businessRequests(requests)).toEqual(beforeReply);
            const command = requests.find((request) => request.path === CONFIRM_POST);
            if (command === undefined) throw new Error("未发送确认证明");
            const body = bodyOf(command);
            expect(body).toMatchObject({
              command_id: commandId,
              expected_fencing_version: 4,
              evidence_reference: "mailbox_sweep:7000001",
              reason: "已核对 Sent 实见",
            });
            expect(body).not.toHaveProperty("accept_collected_assets");
            expect(body).not.toHaveProperty("accept_attempt_history");
            confirm.resolve(
              jsonReply({
                code: "ok",
                message: "人工 Sent 证明已提交",
                data: {
                  command_id: commandId,
                  delivery_id: DELIVERY_ID,
                  external_reference: "mooglemail:main:send:7000001",
                  replayed: false,
                },
              }),
            );
            await settle();
            expect(businessRequests(requests)).toEqual([...beforeReply, `GET ${DETAIL_GET}`]);
            expect(dom.hasExactText(UNAVAILABLE)).toBe(true);
            expect(dom.hasExactText(SENT_RECEIPT)).toBe(true);
            expect(dom.findButton(OPEN)).toBeNull();
            dom.button("重试").click();
            await settle();
            expect(businessRequests(requests)).toEqual([
              ...beforeReply,
              `GET ${DETAIL_GET}`,
              `GET ${DETAIL_GET}`,
            ]);
            expect(dom.hasExactText(UNAVAILABLE)).toBe(false);
            expect(dom.hasExactText(SENT_RECEIPT)).toBe(true);
          }),
      ),
    );
  } finally {
    commandUuid.mockRestore();
  }
});

test("stale_fence_on_confirmation_reloads_detail_and_removes_old_evidence", async () => {
  const { detailPage } = await DELIVERY_RENDER.detail();
  const initial = detailFixtureWire();
  const { detail } = detailFixture().data;
  const oldReference = firstOf(detail.evidence).reference;
  const refreshed = structuredClone(initial);
  setAt(refreshed, "/data/detail/delivery/status", "failed");
  setAt(refreshed, "/data/detail/delivery/fencing_version", detail.delivery.fencing_version + 1);
  setAt(refreshed, "/data/detail/evidence", []);
  let reads = 0;
  await withTestTimezone("UTC", () =>
    withHttp(
      (request) => {
        if (request.path === DETAIL_GET) {
          reads += 1;
          return jsonReply(reads === 1 ? initial : refreshed);
        }
        if (request.path === PREVIEW_POST)
          return previewReply(previewData(DELIVERY_ID, 4, "7000001"));
        if (request.path === CONFIRM_POST) return staleDeliveryReply();
        throw new Error(`未预期的请求 ${request.path}`);
      },
      async (requests) =>
        withDeliveryDom(PAGE, async (dom, mount) => {
          mount(() => detailPage(DELIVERY_ID));
          await settle();
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`]);
          expect(dom.hasExactText(oldReference)).toBe(true);
          const form = await openReviewedProof(dom);
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`, `POST ${PREVIEW_POST}`]);
          form.submit();
          await settle();
          expect(businessRequests(requests)).toEqual([
            `GET ${DETAIL_GET}`,
            `POST ${PREVIEW_POST}`,
            `POST ${CONFIRM_POST}`,
            `GET ${DETAIL_GET}`,
          ]);
          expect(dom.hasExactText(oldReference)).toBe(false);
          expect(dom.findButton(OPEN)).toBeNull();
          expect(dom.findButton(CONFIRM)).toBeNull();
          expect(
            dom.hasExactText(
              "投递状态或证据已变化，请核对最新详情后重试。提交 Sent 证明时请重新选择证据。",
            ),
          ).toBe(true);
        }),
    ),
  );
});

test("delivery_reason_fields_reject_invalid_input_and_acknowledgement_retries_the_frozen_reason", async () => {
  const { detailPage } = await DELIVERY_RENDER.detail();
  const acknowledge = `/api/v1/admin/deliveries/${DELIVERY_ID}/acknowledge`;
  const wire = acknowledgeFixture();
  const boundary = `${"界".repeat(682)}ab`;
  const inputs = [
    ["\u0085\u3000", "请填写此字段。"],
    ["界".repeat(683), "请输入不超过 2048 个 UTF-8 字节的内容。"],
    ["核\n对", "请删除换行、制表符等控制字符。"],
    ["核\t对", "请删除换行、制表符等控制字符。"],
    ["核\u007f对", "请删除换行、制表符等控制字符。"],
  ] as const;
  let acknowledgements = 0;
  await withTestTimezone("UTC", () =>
    withHttp(
      (request) => {
        if (request.path === DETAIL_GET) return jsonReply(wire);
        if (request.path === PREVIEW_POST)
          return previewReply(previewData(DELIVERY_ID, 4, "7000001"));
        if (request.path === acknowledge) {
          return jsonReply({
            code: "ok",
            message: "已处置",
            data: {
              delivery_id: DELIVERY_ID,
              fencing_version: acknowledgements++ === 0 ? 3 : 4,
              kind: "operator_acknowledged",
            },
          });
        }
        if (request.path.startsWith("/api/v1/browser-errors?"))
          return new Response(null, { status: 204 });
        throw new Error(`未预期的请求 ${request.path}`);
      },
      (requests) =>
        withDeliveryDom(PAGE, async (dom, mount) => {
          mount(() => detailPage(DELIVERY_ID));
          await settle();
          const form = await openReviewedProof(dom);
          const fields = [
            {
              field: dom.byId("delivery-sent-reason"),
              id: "delivery-sent-reason-error",
              button: CONFIRM,
              attempt: () => form.submit(),
            },
            {
              field: dom.need(
                (node) => node.tag === "textarea" && node.getAttribute("rows") === "3",
              ),
              id: "delivery-acknowledge-reason-error",
              button: "确认已处置",
              attempt: () => dom.button("确认已处置").dispatchClick(),
            },
          ];
          for (const { field, id, button, attempt } of fields) {
            for (const [input, message] of inputs) {
              field.input(input);
              await settle();
              expect(field.getAttribute("aria-invalid")).toBe("true");
              expect(field.getAttribute("aria-describedby")).toBe(id);
              expect(dom.byId(id).textContent).toBe(message);
              expect(dom.button(button).disabled).toBe(true);
              attempt();
              await settle();
              expect(businessRequests(requests)).toEqual([
                `GET ${DETAIL_GET}`,
                `POST ${PREVIEW_POST}`,
              ]);
            }
            field.input(`\u0085${boundary}\u3000`);
            await settle();
            expect(field.getAttribute("aria-invalid")).toBe("false");
            expect(field.getAttribute("aria-describedby")).toBeNull();
            expect(dom.find((node) => node.id === id)).toBeNull();
            expect(dom.button(button).disabled).toBe(false);
          }
          dom.button("取消").click();
          dom.button("确认已处置").click();
          await settle();
          expect(dom.hasExactText("已确认该投递失败为已处置。")).toBe(false);
          expect(dom.button("确认已处置").disabled).toBe(true);
          dom.button("原样重试").click();
          await settle();
          expect(dom.hasExactText("已确认该投递失败为已处置。")).toBe(true);
          expect(dom.findButton("原样重试")).toBeNull();
          const writes = requests.filter(({ path }) => path === acknowledge);
          expect(writes).toHaveLength(2);
          expect(writes[1]).toEqual(writes[0]);
          expect(bodyOf(firstOf(writes))).toEqual({
            command_id: expect.any(String),
            expected_fencing_version: 4,
            reason: boundary,
          });
        }),
    ),
  );
});

test("sent_proof_preview_sends_no_confirmation_and_mismatched_echo_stays_pending", async () => {
  const { detailPage } = await DELIVERY_RENDER.detail();
  const wire = detailFixtureWire();
  let previews = 0;
  let confirmations = 0;
  const boundary = `${"界".repeat(682)}ab`;
  await withTestTimezone("UTC", () =>
    withHttp(
      (request) => {
        if (request.path === DETAIL_GET) return jsonReply(wire);
        if (request.path === PREVIEW_POST) {
          if (previews++ === 0) return unavailableReply();
          return previewReply(previewData(DELIVERY_ID, 4, "7000001"));
        }
        if (request.path === CONFIRM_POST) {
          return jsonReply({
            code: "ok",
            message: "人工 Sent 证明已提交",
            data: {
              command_id:
                confirmations++ === 0
                  ? "00000000-0000-4000-8000-000000000099"
                  : bodyOf(request).command_id,
              delivery_id: DELIVERY_ID,
              external_reference: "mooglemail:main:send:7000001",
              replayed: false,
            },
          });
        }
        if (request.path.startsWith("/api/v1/browser-errors?"))
          return new Response(null, { status: 204 });
        throw new Error(`未预期的请求 ${request.path}`);
      },
      (requests) =>
        withDeliveryDom(PAGE, async (dom, mount) => {
          mount(() => detailPage(DELIVERY_ID));
          await settle();
          dom.button(OPEN).click();
          await settle();
          dom.byId("delivery-sent-sender-name").input("Sample");
          dom.byId("delivery-sent-sender-uid").input("12345");
          dom.byId("delivery-sent-minute").input("2026-11-02T01:14");
          dom.byId("delivery-sent-reason").input(`\u0085${boundary}\u3000`);
          dom.button("预览准入").click();
          await settle();
          const form = dom.need(
            (node) =>
              node.tag === "form" && node.getAttribute("class") === "admin-delivery-sent-proof",
          );
          expect(dom.button(CONFIRM).disabled).toBe(true);
          form.submit();
          await settle();
          expect(businessRequests(requests)).toEqual([`GET ${DETAIL_GET}`, `POST ${PREVIEW_POST}`]);
          expect(dom.findButton("原样重试")).toBeNull();
          dom.button("预览准入").click();
          await settle();
          expect(dom.button(CONFIRM).disabled).toBe(false);
          form.submit();
          await settle();
          expect(dom.hasExactText(SENT_RECEIPT)).toBe(false);
          expect(dom.button("原样重试").disabled).toBe(false);
          dom.button("原样重试").click();
          await settle();
          expect(dom.hasExactText(SENT_RECEIPT)).toBe(true);
          expect(dom.findButton("原样重试")).toBeNull();
          const writes = requests.filter(({ path }) => path === CONFIRM_POST);
          expect(writes).toHaveLength(2);
          expect(writes[1]).toEqual(writes[0]);
          expect(bodyOf(firstOf(writes))).toMatchObject({
            command_id: expect.any(String),
            expected_fencing_version: 4,
            reason: boundary,
            evidence_reference: "mailbox_sweep:7000001",
          });
        }),
    ),
  );
});

test("delivery_detail_history_loads_the_explicit_id", async () => {
  const { DeliveryRouteFixture } = await DELIVERY_RENDER.detailWithList();
  const other = "00000000-0000-4000-8000-000000000031";
  const otherGet = `/api/v1/admin/deliveries/${other}?include=sent_proof`;
  const enter = Promise.withResolvers<() => void>();
  await withHttp(
    (request) => {
      const wire = detailFixtureWire();
      if (request.path === DETAIL_GET) return jsonReply(wire);
      if (request.path === otherGet) {
        setAt(wire, "/data/detail/delivery/id", other);
        return jsonReply(wire);
      }
      throw new Error(`未预期的请求 ${request.path}`);
    },
    (requests) =>
      withDeliveryDom(PAGE, async (dom, mount) => {
        mount(() => {
          const router = useRouter();
          enter.resolve(() => router.push(`/deliveries/${other}`));
          return createComponent(DeliveryRouteFixture, {});
        });
        const next = await enter.promise;
        await settle();
        expect(dom.hasExactText(DELIVERY_ID)).toBe(true);
        next();
        await settle();
        expect(dom.hasExactText(other)).toBe(true);
        window.history.back();
        await settle();
        expect(dom.hasExactText(DELIVERY_ID)).toBe(true);
        expect(dom.hasExactText(other)).toBe(false);
        expect(businessRequests(requests)).toEqual([
          `GET ${DETAIL_GET}`,
          `GET ${otherGet}`,
          `GET ${DETAIL_GET}`,
        ]);
      }),
  );
});
