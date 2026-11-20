// 结算报告与恢复确认框的加载状态、键盘关闭及焦点回退。

import { expect, jest, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";

import {
  installFixtureFetch,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { adminContract } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { createComponent, createElement, insert, insertNode, settle } from "../solid-dom-fixture";
import { currentTestDocument } from "../test-dom-fixture";
import { requireElement, withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const MAIN = "/settlement?world=main&page_size=10";
const CATALOG = "/api/v1/admin/settlement-catalog";
const REPORT = `/api/v1/admin/auctions/${AUCTION_ID}/settlement-report`;
const RECOVERY = `/api/v1/admin/auctions/${AUCTION_ID}/recovery-report`;
const RECOVERY_BODY = adminContract("recovery-report")
  .replaceAll("00000000-0000-4000-8000-000000000002", AUCTION_ID)
  .replace('"allowed_commands": []', '"allowed_commands": ["reclaimed"]');
const CATALOG_REPLY = {
  code: "ok",
  message: "结算目录已加载",
  data: {
    auctions: [
      {
        auction_id: AUCTION_ID,
        world_id: "main",
        sequence: 5,
        settled_at: "2025-11-16T09:46:40.110000Z",
      },
    ],
    pagination: { page: 1, page_size: 10, total: 1 },
  },
};

for (const foregroundModal of [false, true]) {
  test(
    foregroundModal
      ? "结算确认关闭时前景还有模态框，报告标题不抢焦"
      : "结算确认的 Escape 保留已还原的焦点，提交后回退至报告标题",
    async () => {
      const { page } = await loadRenderModule<typeof import("./page")>(
        `${import.meta.dir}/page.tsx`,
        "page",
      );
      await withAdminTree(MAIN, async (tree) => {
        tree.requests.installBrowserRequest();
        const confirmPath = "/api/v1/admin/recoveries/00000000-0000-4000-8000-000000000001/confirm";
        const posted = Promise.withResolvers<Response>();
        const loaded = Promise.withResolvers<Response>();
        const requests: ReturnType<typeof recordRequest>[] = [];
        const restore = installFixtureFetch(async (input, init) => {
          const request = recordRequest(input, init);
          requests.push(request);
          switch (new URL(request.url, "https://auction.test").pathname) {
            case CATALOG:
              return Response.json(CATALOG_REPLY);
            case new URL(REPORT, "https://auction.test").pathname:
              return loaded.promise;
            case new URL(RECOVERY, "https://auction.test").pathname:
              return new Response(RECOVERY_BODY);
            case "/api/v1/admin/session":
              return Response.json(TEST_SESSION_BODY);
            case confirmPath:
              return posted.promise;
            case "/api/v1/browser-errors":
              return new Response(null, { status: 204 });
            default:
              throw new Error(`未预期的请求 ${request.url}`);
          }
        });
        jest.useFakeTimers();
        try {
          tree.mount(page);
          await settle();
          expect(
            tree.elements((node) => node.textContent === "正在加载结算数据……" && node.tag === "h2"),
          ).toHaveLength(1);
          expect(tree.find((node) => node.tag === "tbody")).toBeNull();
          loaded.resolve(new Response(adminContract("settlement-report")));
          await settle();
          expect(
            tree.find((node) => node.tag === "td" && node.textContent === "1h 00m"),
          ).not.toBeNull();
          const trigger = requireElement(
            tree.find((node) => node.tag === "button" && node.textContent === "确认已回收"),
            "恢复操作",
          );
          const title = requireElement(
            tree.find((node) => node.id === "settlement-report-panel-title"),
            "结算标题",
          );
          trigger.focus();
          trigger.click();
          await settle();
          const escapeTarget = requireElement(
            tree.find((node) => node.matches(".ui-dialog")),
            "对话框遮罩",
          );
          const onKeyDown = escapeTarget.attributes.onKeyDown;
          if (typeof onKeyDown !== "function") throw new Error("确认框未绑定 Escape");
          const escapeEvent = Object.assign(new Event("keydown", { cancelable: true }), {
            key: "Escape",
          });
          onKeyDown(escapeEvent);
          expect(escapeEvent.defaultPrevented).toBe(true);
          await settle();
          jest.advanceTimersByTime(0);
          await settle();
          expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();
          expect(currentTestDocument()?.activeElement).toBe(trigger);
          expect(title.focusCount).toBe(0);

          trigger.click();
          await settle();
          const form = requireElement(
            tree.find((node) => node.matches(".admin-auction-recovery-confirm")),
            "恢复确认表单",
          );
          const fixture = currentTestDocument();
          if (fixture === null) throw new Error("没有内存文档");
          const find = fixture.querySelectorAll.bind(fixture);
          const front = createElement("section", { role: "dialog", "aria-modal": "true" });
          const modalList = spyOn(fixture, "querySelectorAll").mockImplementation((selector) => {
            const matches = find(selector);
            if (foregroundModal && selector === '[role="dialog"][aria-modal="true"]') {
              matches.push(front);
            }
            // 内存 DOM 返回数组，生产的模态判定读取 NodeList.item。
            return Object.assign(matches, { item: (index: number) => matches[index] ?? null });
          });
          try {
            fixture.activeElement = null;
            expect(form.submit()).toBe(true);
            await settle();
            jest.advanceTimersByTime(0);
            await settle();
            expect(trigger.disabled).toBe(true);
            expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();
            expect(currentTestDocument()?.activeElement).toBe(foregroundModal ? null : title);
            expect(title.focusCount).toBe(foregroundModal ? 0 : 1);
          } finally {
            modalList.mockRestore();
          }
          expect(requests.filter((request) => request.method === "POST")).toHaveLength(1);
        } finally {
          jest.useRealTimers();
          posted.resolve(new Response("{", { status: 409 }));
          await settle();
          restore();
        }
      });
    },
  );
}

for (const focusEntered of [false, true]) {
  test(`结算报告重试成功时，前景模态框${focusEntered ? "已持有焦点" : "正在接管焦点"}仍阻止报告标题抢焦`, async () => {
    const { page } = await loadRenderModule<typeof import("./page")>(
      `${import.meta.dir}/page.tsx`,
      "page",
    );
    await withAdminTree(MAIN, async (tree) => {
      const pending = Promise.withResolvers<Response>();
      const requests: ReturnType<typeof recordRequest>[] = [];
      let reportReads = 0;
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        switch (new URL(request.url, "https://auction.test").pathname) {
          case CATALOG:
            return Response.json(CATALOG_REPLY);
          case REPORT:
            reportReads += 1;
            return reportReads === 1
              ? Response.json(
                  {
                    code: "temporarily_unavailable",
                    message: "暂不可用",
                    data: null,
                    request_id: null,
                  },
                  { status: 503 },
                )
              : pending.promise;
          case RECOVERY:
            return new Response(RECOVERY_BODY);
          default:
            throw new Error(`未预期的请求 ${request.url}`);
        }
      });
      const document = currentTestDocument();
      if (document === null) throw new Error("没有内存文档");
      const query = document.querySelectorAll.bind(document);
      const modalList = spyOn(document, "querySelectorAll").mockImplementation((selector) => {
        const matches =
          selector === '[role="dialog"][aria-modal="true"]'
            ? tree.elements(
                (node) =>
                  node.getAttribute("role") === "dialog" &&
                  node.getAttribute("aria-modal") === "true",
              )
            : query(selector);
        return Object.assign(matches, { item: (index: number) => matches[index] ?? null });
      });
      try {
        const open = createCell(false);
        const dialog = createElement("section", { role: "dialog", "aria-modal": "true" });
        const modalButton = createElement("button");
        insertNode(dialog, modalButton);
        tree.mount(() => {
          const foreground = createElement("div");
          insert(foreground, () => (open() ? dialog : null));
          return [createComponent(page, {}), foreground];
        });
        await settle();
        const retry = requireElement(
          tree.find((node) => node.id === "settlement-report-retry"),
          "结算报告重试按钮",
        );
        const heading = requireElement(
          tree.find((node) => node.id === "settlement-report-panel-title"),
          "结算报告标题",
        );
        retry.focus();
        retry.click();
        await settle();
        expect(reportReads).toBe(2);
        expect(retry.getAttribute("aria-disabled")).toBe("true");
        open.set(true);
        await settle();
        expect(tree.find((node) => node === dialog)).toBe(dialog);
        // 遮罩挂载比焦点交接早时，旧按钮仍可能持有焦点；这时也不能把焦点交给背景标题。
        if (focusEntered) modalButton.focus();
        expect(document.activeElement).toBe(focusEntered ? modalButton : retry);
        pending.resolve(new Response(adminContract("settlement-report")));
        await settle();
        expect(tree.find((node) => node.id === "settlement-report-retry")).toBeNull();
        expect(
          tree.find((node) => node.tag === "td" && node.textContent === "1h 00m"),
        ).not.toBeNull();
        expect(heading.focusCount).toBe(0);
        if (focusEntered) {
          expect(document.activeElement).toBe(modalButton);
          expect(modalButton.parentElement).toBe(dialog);
        }
        expect(requests.filter(({ url }) => url === REPORT)).toEqual([
          { method: "GET", url: REPORT, body: "" },
          { method: "GET", url: REPORT, body: "" },
        ]);
      } finally {
        pending.resolve(new Response(adminContract("settlement-report")));
        await settle();
        modalList.mockRestore();
        restore();
      }
    });
  });
}
