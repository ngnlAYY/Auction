// 退件详情的渲染、焦点回退与命令结果引起的读取。

import { expect, jest, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { Show } from "solid-js";

import {
  installFixtureFetch,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { CASE, DELIVERY, returnDetail } from "../../api/returns-fixture";
import { adminContract } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { type PendingCommandHost, requirePendingCommandHost } from "../pending";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { createComponent, settle } from "../solid-dom-fixture";
import { currentTestDocument } from "../test-dom-fixture";

const DETAIL = `/api/v1/admin/settlement-returns/${CASE}`;
const HANDLE = `${DETAIL}/handle`;
const CONFIRMED_SESSION = "/api/v1/admin/session";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];

function detailReply() {
  return new Response(
    JSON.stringify({ code: "ok", message: "案件已加载", data: returnDetail().data }),
  );
}

function button(tree: Tree, text: string, withinDialog = false) {
  const scope = withinDialog
    ? requireElement(
        tree.find((node) => node.getAttribute("role") === "dialog"),
        "确认框",
      )
    : null;
  return requireElement(
    (scope === null
      ? tree.elements((node) => node.tag === "button")
      : scope.querySelectorAll("button")
    ).find((node) => node.textContent === text && node.getAttribute("aria-label") === null) ?? null,
    text,
  );
}

function address(request: ReturnType<typeof recordRequest>) {
  return new URL(request.url, "https://auction.test").pathname;
}

async function runFocusFallback() {
  await settle();
  jest.advanceTimersByTime(0);
  await settle();
}

test("退件详情从真实 GET 渲染资产、历史事件及处理入口，读取失败时不显示旧详情", async () => {
  const { detailPage } = await loadRenderModule<Pick<typeof import("./detail"), "detailPage">>(
    `${import.meta.dir}/detail.tsx`,
    "detailPage",
  );
  await withAdminTree(`/settlement-returns/${CASE}`, async (tree) => {
    const failure = {
      status: 503,
      body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
    };
    const requests = tree.requests.installFetch([{ body: await detailReply().text() }, failure]);
    tree.mount(() => detailPage(CASE));
    await settle();
    expect(tree.find((node) => node.tag === "dd" && node.textContent === "Seller")).not.toBeNull();
    expect(tree.find((node) => node.tag === "td" && node.textContent === "建案")).not.toBeNull();
    expect(button(tree, "领回并补寄").disabled).toBe(false);
    button(tree, "刷新案件").click();
    await settle();
    expect(tree.find((node) => node.tag === "dd" && node.textContent === "Seller")).toBeNull();
    expect(
      tree.find((node) => node.tag === "button" && node.textContent === "领回并补寄"),
    ).toBeNull();
    expect(requests).toEqual([
      { url: DETAIL, method: "GET", body: "" },
      { url: DETAIL, method: "GET", body: "" },
    ]);
  });
});

for (const outcome of ["写入拒绝", "写前网络失败", "无效成功回执"] as const) {
  test(`退件${outcome}后按显示码决定重读并保留准确的重试行为`, async () => {
    const { detailPage } = await loadRenderModule<Pick<typeof import("./detail"), "detailPage">>(
      `${import.meta.dir}/detail.tsx`,
      "detailPage",
    );
    await withAdminTree(`/settlement-returns/${CASE}`, async (tree) => {
      tree.requests.installBrowserRequest();
      const requests: ReturnType<typeof recordRequest>[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        switch (address(request)) {
          case DETAIL:
            return detailReply();
          case CONFIRMED_SESSION:
            if (outcome === "写前网络失败") throw new TypeError("内存会话传输中断");
            return Response.json(TEST_SESSION_BODY);
          case HANDLE:
            return outcome === "写入拒绝"
              ? new Response(adminContract("error-stale-settlement-return-state"), { status: 409 })
              : new Response(adminContract("settlement-return-command"));
          case "/api/v1/browser-errors":
            return new Response(null, { status: 204 });
          default:
            throw new Error(`未预期的请求 ${request.url}`);
        }
      });
      jest.useFakeTimers();
      try {
        tree.mount(() => detailPage(CASE));
        await settle();
        const trigger = button(tree, "领回并补寄");
        trigger.focus();
        trigger.click();
        button(tree, "取消", true).click();
        // 模拟共享焦点陷阱已经还焦给未被禁用的触发按钮。
        await runFocusFallback();
        expect(currentTestDocument()?.activeElement).toBe(trigger);
        expect(requests.map(address)).toEqual([DETAIL]);

        trigger.click();
        const confirm = button(tree, "确认提交", true);
        const documentFixture = currentTestDocument();
        if (documentFixture === null) throw new Error("没有内存文档");
        // 模拟浏览器在提交后失去弹窗焦点，触发按钮在途禁用或被重读卸载。
        documentFixture.activeElement = null;
        confirm.click();
        await runFocusFallback();
        expect(currentTestDocument()?.activeElement).toBe(
          tree.find((node) => node.id === "settlement-return-detail-title"),
        );
        expect(tree.find((node) => node.getAttribute("role") === "dialog")).toBeNull();

        const detailReads = () => requests.filter((request) => address(request) === DETAIL);
        const posts = () => requests.filter((request) => address(request) === HANDLE);
        // 了结时宿主仍通知匹配的详情一次，写前失败不再由退件回调单独推进。
        expect(detailReads()).toHaveLength(outcome === "无效成功回执" ? 1 : 2);
        expect(requests.filter((request) => address(request) === CONFIRMED_SESSION)).toHaveLength(
          1,
        );
        if (outcome === "写前网络失败") {
          expect(posts()).toEqual([]);
        } else {
          const [post] = posts();
          if (post === undefined) throw new Error("没有退件写入");
          expect(post.method).toBe("POST");
          expect(post.body).toMatch(/"command_id":"[0-9a-f-]{36}"/u);
          expect(post.body).toContain('"expected_version":2');
          expect(post.body).toContain('"action":"reclaim_and_resend"');
        }
        if (outcome === "无效成功回执") {
          const first = posts()[0];
          if (first === undefined) throw new Error("没有可重试的请求");
          button(tree, "原样重试").click();
          await settle();
          expect(posts()).toEqual([first, first]);
          expect(detailReads()).toHaveLength(1);
        } else {
          expect(
            tree.find((node) => node.tag === "button" && node.textContent === "原样重试"),
          ).toBeNull();
        }
      } finally {
        jest.useRealTimers();
        restore();
      }
    });
  });
}

for (const valid of [true, false]) {
  test(`退件写入在途离开详情，${valid ? "成功回执交给外壳" : "错案件回执保留原命令供显式重试"}`, async () => {
    const { detailPage } = await loadRenderModule<Pick<typeof import("./detail"), "detailPage">>(
      `${import.meta.dir}/detail.tsx`,
      "detailPage",
    );
    const { PendingCommandNotices } = await loadRenderModule<
      Pick<typeof import("../pending/panel"), "PendingCommandNotices">
    >(new URL("../pending/panel.tsx", import.meta.url).pathname, "PendingCommandNotices");
    const href = `/settlement-returns/${CASE}`;
    await withAdminTree(href, async (tree) => {
      tree.requests.installBrowserRequest();
      const posted = Promise.withResolvers<Response>();
      const requests: ReturnType<typeof recordRequest>[] = [];
      const posts = () =>
        requests.filter((request) => request.method === "POST" && address(request) === HANDLE);
      const receipt = (commandId: string, returnId: string) =>
        Response.json({
          code: "ok",
          message: "退件动作已受理",
          data: {
            command_id: commandId,
            return_id: returnId,
            action: "reclaim_and_resend",
            case_version: 3,
            status: "take_pending",
          },
        });
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        switch (address(request)) {
          case DETAIL:
            return detailReply();
          case CONFIRMED_SESSION:
            return Response.json(TEST_SESSION_BODY);
          case HANDLE:
            if (posts().length === 1) return posted.promise;
            return receipt(readCommandId(request.body), CASE);
          case "/api/v1/browser-errors":
            return new Response(null, { status: 204 });
          default:
            throw new Error(`未预期的请求 ${request.url}`);
        }
      });
      const observed: { host: PendingCommandHost | null } = { host: null };
      const title = () => tree.find((node) => node.id === "settlement-return-detail-title");
      try {
        tree.mount(() => {
          const host = requirePendingCommandHost();
          observed.host = host;
          return [
            createComponent(PendingCommandNotices, { host }),
            createComponent(Show, {
              get when() {
                return tree.router.currentLocationString() === href;
              },
              children: () => detailPage(CASE),
            }),
          ];
        });
        await settle();
        const host = observed.host;
        if (host === null) throw new Error("未挂载命令宿主");
        const originalTitle = requireElement(title(), "来源详情标题");
        button(tree, "领回并补寄").click();
        button(tree, "确认提交", true).click();
        await settle();
        const [first] = posts();
        if (first === undefined) throw new Error("未发出退件 POST");
        expect(posts()).toEqual([{ url: HANDLE, method: "POST", body: first.body }]);
        const commandId = readCommandId(first.body);
        expect(commandId).toMatch(/^[0-9a-f-]{36}$/u);
        expect(first.body).toContain('"expected_version":2');
        expect(first.body).toContain('"action":"reclaim_and_resend"');

        tree.router.push("/settlement-returns");
        await settle();
        expect(title()).toBeNull();
        expect(host.entries.peek().map(({ identity, busy }) => [identity.commandId, busy])).toEqual(
          [[commandId, true]],
        );
        posted.resolve(receipt(commandId, valid ? CASE : DELIVERY));
        await settle();
        expect(title()).toBeNull();
        expect(
          host.notices.peek().map(({ identity, result }) => [identity.commandId, result]),
        ).toEqual(valid ? [[commandId, { ok: true, value: null }]] : []);
        expect(
          host.entries
            .peek()
            .map(({ identity, busy, unknown }) => [identity.commandId, busy, unknown]),
        ).toEqual(valid ? [] : [[commandId, false, true]]);

        tree.router.goBack();
        await settle();
        expect(requireElement(title(), "重新进入的详情标题")).not.toBe(originalTitle);
        expect(
          requests.filter(({ url, method }) => url === DETAIL && method === "GET"),
        ).toHaveLength(2);
        expect(posts()).toEqual([first]);
        if (valid) {
          expect(
            tree.find((node) => node.tag === "button" && node.textContent === "原样重试"),
          ).toBeNull();
        } else {
          expect(button(tree, "领回并补寄").disabled).toBe(true);
          button(tree, "原样重试").click();
          await settle();
          expect(posts()).toEqual([first, first]);
          expect(host.entries.peek()).toEqual([]);
        }
      } finally {
        posted.resolve(new Response("{", { status: 409 }));
        await settle();
        restore();
      }
    });
  });
}

function readCommandId(body: string): string {
  const parsed = parseJson(body);
  if (
    !parsed.ok ||
    typeof parsed.value !== "object" ||
    parsed.value === null ||
    !("command_id" in parsed.value) ||
    typeof parsed.value.command_id !== "string"
  ) {
    throw new Error("退件正文缺少 command_id");
  }
  return parsed.value.command_id;
}
