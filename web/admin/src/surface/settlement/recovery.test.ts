// 恢复表格的移动卡片标签、旧报告事件守卫与退款后果提示。

import { expect, spyOn, test } from "bun:test";
import { AdminRecoveryResponseSchema } from "@auction/contracts/dto/admin/recovery";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { flush, createComponent as mountComponent } from "solid-js";
import {
  installFixtureFetch,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { adminContract, requireOk } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  type PendingCommandHost,
  PendingCommandsContext,
  requirePendingCommandHost,
} from "../pending";
import { usePendingCommands } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { createComponent, createElement, insert, settle } from "../solid-dom-fixture";
import type { RecoveryConfirmationDraft, RecoveryRead } from "./recovery";
import { cardLabels, requireElement, withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "00000000-0000-4000-8000-000000000002";
const RECOVERY_ID = "00000000-0000-4000-8000-000000000001";
const REFUND_TEXT = "确认“装备已恢复”还会激活买家退款，并关闭原卖家货款义务。";

function recoveryRow() {
  return requireOk(
    decode(AdminRecoveryResponseSchema, {
      id: RECOVERY_ID,
      world_id: "main",
      auction_id: AUCTION_ID,
      lot_number: 1,
      lot_id: "00000000-0000-4000-8000-000000000003",
      equipment_name: "Phase Robe",
      equipment_category: "Cloth",
      stage: "reclaiming",
      resolution: null,
      state_version: 2,
      payment_deadline_at: "2027-01-01T00:00:00Z",
      reminder_count: 0,
      reminder_limit: 1,
      custody_proof_kind: null,
      custody_proof_observed_at: null,
      blocking_code: null,
      manual_reconcile_delivery_id: null,
      relisted_auction_id: null,
      relisted_lot_id: null,
      actions: [],
      allowed_commands: ["reclaimed"],
      updated_at: "2027-01-01T00:00:00Z",
    }),
  );
}

function loadRecovery() {
  return loadRenderModule<typeof import("./recovery")>(
    `${import.meta.dir}/recovery.tsx`,
    "AdminRecoveryTable",
    "AdminRecoveryDialog",
  );
}

async function withRecoveryTable(run: Parameters<typeof withAdminTree>[1]) {
  const { AdminRecoveryTable } = await loadRecovery();
  const { useAdminPagination } = await loadRenderModule<
    Pick<typeof import("../pagination"), "useAdminPagination">
  >(new URL("../pagination/index.tsx", import.meta.url).pathname, "useAdminPagination");
  await withAdminTree("/settlement?recovery_page_size=10", async (tree) => {
    const report = { auction_id: AUCTION_ID, recoveries: [recoveryRow()] };
    tree.mount(() => {
      const reload = createCell(0);
      const loaded = createCell(0);
      const page = createCell(1);
      const confirmation = createCell<RecoveryConfirmationDraft | null>(null);
      const pagination = useAdminPagination(
        "settlement-recovery",
        ["recovery_page_size"],
        null,
        100_000,
      );
      const pending = usePendingCommands(createCell(0n), () => true);
      const invalidate = createElement("button", { onClick: () => reload.set(1) });
      insert(invalidate, "使恢复报告失效");
      const finish = createElement("button", { onClick: () => loaded.set(1) });
      insert(finish, "完成恢复报告读取");
      const draft = createElement("p");
      insert(draft, () => (confirmation() === null ? "" : "已有恢复确认草稿"));
      return createComponent(PendingCommandsContext, {
        value: pending,
        get children() {
          return [
            invalidate,
            finish,
            createComponent(AdminRecoveryTable, {
              report,
              currentPage: 1,
              pagination,
              page,
              actionBusy: () => false,
              get recoveryRead(): RecoveryRead {
                return {
                  reload,
                  recoveryReload: reload,
                  generation: [loaded(), loaded()],
                  alive: () => true,
                };
              },
              confirmation,
              reason: createCell(""),
              actionFeedback: createCell<AdminActionFeedback | null>(null),
            }),
            draft,
          ];
        },
      });
    });
    await settle();
    await run(tree);
  });
}

test("every_recovery_cell_renders_its_card_label", async () => {
  await withRecoveryTable((tree) => {
    const body = requireElement(
      tree.find((node) => node.tag === "tbody"),
      "恢复表体",
    );
    expect(body.querySelectorAll("tr")).toHaveLength(1);
    expect(cardLabels(body)).toEqual(
      new Set(["拍品", "装备", "恢复阶段", "付款截止", "证据/阻塞原因", "恢复操作", "操作"]),
    );
  });
});

test("recovery_entry_rejects_events_from_an_invalidated_report", async () => {
  await withRecoveryTable(async (tree) => {
    const button = (label: string) => {
      const target = requireElement(
        tree.find((node) => node.tag === "button" && node.textContent === label),
        label,
      );
      expect(typeof target.attributes.onClick, label).toBe("function");
      return target;
    };
    expect(button("确认已回收").hasAttribute("disabled")).toBe(false);

    const clickEnabled = (label: string) => {
      const target = button(label);
      expect(target.hasAttribute("disabled"), label).toBe(false);
      target.click();
    };
    clickEnabled("使恢复报告失效");
    await settle();
    const stale = button("确认已回收");
    expect(stale.hasAttribute("disabled")).toBe(true);
    // 直接派发旧按钮事件，核对事件处理器本身拒绝失效的报告。
    stale.dispatchClick();
    await settle();
    expect(
      tree.find((node) => node.tag === "p" && node.textContent === "已有恢复确认草稿"),
    ).toBeNull();

    clickEnabled("完成恢复报告读取");
    await settle();
    clickEnabled("确认已回收");
    await settle();
    expect(
      tree.find((node) => node.tag === "p" && node.textContent === "已有恢复确认草稿"),
    ).not.toBeNull();
  });
});

test("refund_consequences_apply_only_to_recovered_equipment", async () => {
  const { AdminRecoveryDialog } = await loadRecovery();
  await withAdminTree("/settlement", async (tree) => {
    const action = createCell("equipment_recovered");
    tree.mount(() => {
      const pending = usePendingCommands(createCell(0n), () => true);
      const reload = createCell(0);
      return createComponent(PendingCommandsContext, {
        value: pending,
        get children() {
          return createComponent(AdminRecoveryDialog, {
            get draft() {
              return {
                action: action(),
                auctionId: AUCTION_ID,
                recoveryId: RECOVERY_ID,
                stateVersion: 2,
                equipmentName: "Phase Robe",
                lotCode: "C1",
              };
            },
            confirmation: createCell<RecoveryConfirmationDraft | null>(null),
            reason: createCell(""),
            busy: () => false,
            feedback: createCell<AdminActionFeedback | null>(null),
            selectedAuctionId: createCell(AUCTION_ID),
            selectionGeneration: createCell(0),
            recoveryRead: { reload, recoveryReload: reload, generation: [0, 0], alive: () => true },
            recoveryReloadGeneration: reload,
          });
        },
      });
    });
    await settle();
    const findDialog = () => tree.find((node) => node.getAttribute("role") === "dialog");
    const dialog = requireElement(findDialog(), "确认对话框");
    const consequence = () =>
      tree.find((node) => node.tag === "p" && node.textContent === REFUND_TEXT);
    const changeAction = async (next: string) => {
      action.set(next);
      flush();
      await settle();
      expect(findDialog(), next).toBe(dialog);
    };
    expect(consequence()).not.toBeNull();
    await changeAction("reclaimed");
    expect(consequence()).toBeNull();
    await changeAction("equipment_recovered");
    expect(consequence()).not.toBeNull();
    await changeAction("cancelled");
    expect(consequence()).toBeNull();
  });
});

const CONFIRM_PATH = `/api/v1/admin/recoveries/${RECOVERY_ID}/confirm`;

function confirmationReply(stateVersion = 3) {
  return Response.json({
    code: "ok",
    message: "恢复结论已确认",
    data: {
      recovery: {
        id: RECOVERY_ID,
        action: "reclaimed",
        resolution: "reclaimed",
        stage: "relist_pending",
        state_version: stateVersion,
        relisted_auction_id: null,
        relisted_lot_id: null,
      },
    },
  });
}

for (const scenario of [
  "当前选择收到坏 409",
  "当前选择收到完整 422",
  "当前选择收到 409 响应头后正文中断",
  "重新选择同一场次后收到坏 409",
  "写前收到 409",
  "当前选择收到成功回执",
  "换址后收到成功回执",
  "确认打开后选择世代已变",
  "缺少原因",
] as const) {
  test(`恢复确认：${scenario}`, async () => {
    const { AdminRecoveryDialog } = await loadRecovery();
    await withAdminTree("/settlement", async (tree) => {
      const action = scenario === "缺少原因" ? "equipment_recovered" : "reclaimed";
      const reason = createCell("");
      const selected = createCell(AUCTION_ID);
      const generation = createCell(0);
      const reload = createCell(0);
      const recoveryReload = createCell(0);
      const feedback = createCell<AdminActionFeedback | null>(null);
      const draft = createCell<RecoveryConfirmationDraft | null>({
        auctionId: AUCTION_ID,
        recoveryId: RECOVERY_ID,
        stateVersion: 2,
        action,
        equipmentName: "Phase Robe",
        lotCode: "C1",
      });
      const waiting = Promise.withResolvers<Response>();
      const requests: ReturnType<typeof recordRequest>[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        const url = new URL(request.url, "https://auction.test");
        if (url.pathname === "/api/v1/admin/session") {
          return scenario === "写前收到 409"
            ? Response.json(
                { code: "conflict", message: "冲突", data: null, request_id: null },
                { status: 409 },
              )
            : Response.json(TEST_SESSION_BODY);
        }
        if (url.pathname === CONFIRM_PATH) return waiting.promise;
        if (url.pathname === "/api/v1/browser-errors") return new Response(null, { status: 204 });
        throw new Error(`未预期的请求 ${request.url}`);
      });
      tree.requests.installBrowserRequest();
      const warn = spyOn(console, "warn");
      try {
        const host: { current: PendingCommandHost | null } = { current: null };
        const close = createCell(false);
        tree.mount(() => {
          const pending = usePendingCommands(createCell(0n), () => false);
          host.current = pending.host;
          return createComponent(PendingCommandsContext, {
            value: pending,
            get children() {
              return close()
                ? null
                : mountComponent(AdminRecoveryDialog, {
                    draft: {
                      auctionId: AUCTION_ID,
                      recoveryId: RECOVERY_ID,
                      stateVersion: 2,
                      action,
                      equipmentName: "Phase Robe",
                      lotCode: "C1",
                    },
                    confirmation: draft,
                    reason,
                    busy: () => pending.anyBusy(),
                    feedback,
                    selectedAuctionId: selected,
                    selectionGeneration: generation,
                    recoveryRead: { reload, recoveryReload, generation: [0, 0], alive: () => true },
                    recoveryReloadGeneration: recoveryReload,
                  });
            },
          });
        });
        await settle();
        const pending = host.current;
        if (pending === null) throw new Error("未挂载命令宿主");
        const form = requireElement(
          tree.find((node) => node.tag === "form"),
          "恢复确认表单",
        );
        expect(
          tree.find((node) => node.tag === "p" && node.textContent.includes("Phase Robe")),
        ).not.toBeNull();
        expect(
          warn.mock.calls.filter(([message]) => String(message).includes("STRICT_READ_UNTRACKED")),
        ).toEqual([]);
        if (scenario === "确认打开后选择世代已变") {
          generation.set(2);
        }
        expect(form.submit()).toBe(true);
        await settle();
        if (scenario === "缺少原因" || scenario === "确认打开后选择世代已变") {
          expect(requests).toEqual([]);
          expect(draft.peek()).not.toBeNull();
          expect(tree.find((node) => node.tag === "form")).toBe(form);
          if (scenario === "缺少原因") {
            expect(tree.find((node) => node.tag === "textarea")?.getAttribute("aria-invalid")).toBe(
              "true",
            );
          }
          return;
        }
        expect(draft.peek()).toBeNull();
        const posts = () =>
          requests.filter(
            (request) => new URL(request.url, "https://auction.test").pathname === CONFIRM_PATH,
          );
        if (scenario === "写前收到 409") {
          expect(posts()).toEqual([]);
          expect(recoveryReload.peek()).toBe(0);
          expect(pending.entries.peek()).toEqual([]);
          return;
        }
        const [post] = posts();
        if (post === undefined) throw new Error("未提交恢复确认");
        const commandId = pending.entries.peek()[0]?.identity.commandId;
        if (commandId === undefined) throw new Error("宿主未登记命令标识");
        expect(post).toEqual({
          url: CONFIRM_PATH,
          method: "POST",
          body: JSON.stringify({
            command_id: commandId,
            expected_state_version: 2,
            action,
            reason: "",
          }),
        });
        close.set(true);
        if (scenario === "重新选择同一场次后收到坏 409") {
          selected.set(RECOVERY_ID);
          generation.set(1);
          selected.set(AUCTION_ID);
          generation.set(2);
        } else if (scenario === "换址后收到成功回执") {
          tree.router.push(`/settlement?sequence=1&auction=${RECOVERY_ID}`);
          selected.set(RECOVERY_ID);
          generation.set(1);
        }
        if (scenario === "当前选择收到完整 422") {
          waiting.resolve(
            Response.json(
              {
                code: "invalid_recovery_command",
                message: "恢复命令无效",
                data: null,
                request_id: null,
              },
              { status: 422 },
            ),
          );
          await settle();
          expect(recoveryReload.peek()).toBe(0);
          expect(
            pending.entries.peek().map((entry) => [entry.identity.commandId, entry.unknown]),
          ).toEqual([[commandId, true]]);
          expect(feedback.peek()).toEqual({ kind: "error", code: "http_422" });
          expect(posts()).toEqual([post]);
          return;
        }
        if (scenario === "当前选择收到 409 响应头后正文中断") {
          const readStarted = Promise.withResolvers<ReadableStreamDefaultController<Uint8Array>>();
          const body = new ReadableStream<Uint8Array>(
            {
              pull(controller) {
                readStarted.resolve(controller);
              },
            },
            { highWaterMark: 0 },
          );
          waiting.resolve(new Response(body, { status: 409 }));
          const controller = await readStarted.promise;
          expect(recoveryReload.peek()).toBe(0);
          controller.error(new TypeError("内存响应正文传输中断"));
        } else {
          waiting.resolve(
            scenario.includes("409") ? new Response("{", { status: 409 }) : confirmationReply(),
          );
        }
        await settle();
        expect(recoveryReload.peek()).toBe(scenario === "当前选择收到坏 409" ? 1 : 0);
        if (scenario.includes("409")) {
          expect(pending.notices.peek()).toEqual([]);
          expect(
            pending.entries.peek().map((entry) => [entry.identity.commandId, entry.unknown]),
          ).toEqual([[commandId, true]]);
          pending.retry(commandId);
          await settle();
          expect(posts()).toEqual([post, post]);
        } else {
          expect(pending.entries.peek()).toEqual([]);
          if (scenario === "当前选择收到成功回执") {
            expect(feedback.peek()).toEqual({ kind: "success" });
            expect(pending.notices.peek()).toEqual([]);
          } else {
            expect(feedback.peek()).toBeNull();
            expect(
              pending.notices.peek().map(({ identity, result }) => [identity.commandId, result]),
            ).toEqual([[commandId, { ok: true, value: null }]]);
          }
        }
      } finally {
        warn.mockRestore();
        restore();
      }
    });
  });
}

test("结算页 A→B→A 后迟到的成功回执只交给认证外壳一次", async () => {
  const { page } = await loadRenderModule<typeof import("./page")>(
    `${import.meta.dir}/page.tsx`,
    "page",
  );
  const first = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
  const second = "00000000-0000-4000-8000-000000000020";
  const href = (auction: string, sequence: number) =>
    `/settlement?world=main&page_size=10&auction=${auction}&sequence=${sequence}`;
  await withAdminTree(href(first, 5), async (tree) => {
    tree.requests.installBrowserRequest();
    const posted = Promise.withResolvers<Response>();
    const requests: ReturnType<typeof recordRequest>[] = [];
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      requests.push(request);
      switch (new URL(request.url, "https://auction.test").pathname) {
        case "/api/v1/admin/settlement-catalog":
          return Response.json({
            code: "ok",
            message: "结算目录已加载",
            data: {
              auctions: [
                {
                  auction_id: second,
                  sequence: 6,
                  world_id: "main",
                  settled_at: "2025-11-17T09:46:40Z",
                },
                {
                  auction_id: first,
                  sequence: 5,
                  world_id: "main",
                  settled_at: "2025-11-16T09:46:40Z",
                },
              ],
              pagination: { page: 1, page_size: 10, total: 2 },
            },
          });
        case `/api/v1/admin/auctions/${first}/settlement-report`:
          return new Response(adminContract("settlement-report"));
        case `/api/v1/admin/auctions/${second}/settlement-report`:
          return new Response(
            adminContract("settlement-report")
              .replaceAll(first, second)
              .replace('"sequence": 5', '"sequence": 6'),
          );
        case `/api/v1/admin/auctions/${first}/recovery-report`:
          return new Response(
            adminContract("recovery-report")
              .replaceAll(AUCTION_ID, first)
              .replace('"allowed_commands": []', '"allowed_commands": ["reclaimed"]'),
          );
        case `/api/v1/admin/auctions/${second}/recovery-report`:
          return Response.json({
            code: "ok",
            message: "",
            data: { auction_id: second, recoveries: [] },
          });
        case "/api/v1/admin/session":
          return Response.json(TEST_SESSION_BODY);
        case CONFIRM_PATH:
          return posted.promise;
        case "/api/v1/browser-errors":
          return new Response(null, { status: 204 });
        default:
          throw new Error(`未预期的请求 ${request.url}`);
      }
    });
    const host: { current: PendingCommandHost | null } = { current: null };
    try {
      tree.mount(() => {
        host.current = requirePendingCommandHost();
        return createComponent(page, {});
      });
      await settle();
      const pending = host.current;
      if (pending === null) throw new Error("未挂载命令宿主");
      requireElement(
        tree.find((node) => node.tag === "button" && node.textContent === "确认已回收"),
        "恢复操作",
      ).click();
      expect(
        requireElement(
          tree.find((node) => node.matches(".admin-auction-recovery-confirm")),
          "恢复确认表单",
        ).submit(),
      ).toBe(true);
      await settle();
      const posts = () => requests.filter((request) => request.method === "POST");
      const sent = posts();
      expect(sent).toHaveLength(1);
      const commandId = pending.entries.peek()[0]?.identity.commandId;
      if (commandId === undefined) throw new Error("宿主未登记命令标识");

      for (const [auction, sequence] of [
        [second, 6],
        [first, 5],
      ] as const) {
        tree.router.push(href(auction, sequence));
        await settle();
        expect(tree.find((node) => node.id === "settlement-period")?.value).toBe(auction);
      }
      posted.resolve(confirmationReply(5));
      await settle();
      expect(pending.entries.peek()).toEqual([]);
      expect(
        pending.notices.peek().map(({ identity, result }) => [identity.commandId, result]),
      ).toEqual([[commandId, { ok: true, value: null }]]);
      expect(tree.find((node) => node.matches(".action-feedback"))).toBeNull();
      expect(posts()).toEqual(sent);
    } finally {
      posted.resolve(new Response("{", { status: 409 }));
      await settle();
      restore();
    }
  });
});
