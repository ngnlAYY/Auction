// 告警页复现 Rust 的复发、未决重试、筛选与跨路由序列。

import { expect, test } from "bun:test";
import { jsonObjectAt, type RecordedRequest, type Reply } from "../../api/api-request-fixture";
import { adminFixtureBody } from "../../api/t11-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  installT11Fetch,
  t11Buttons,
  t11Reply,
  withT11AdminPage,
} from "../dashboard/t11-page-fixture.test";
import { fireT11Event, oneT11Element, settleT11View } from "../dashboard/t11-solid-fixture.test";
import { createTextNode, findAll, insertNode, TestElement } from "../solid-dom-fixture";

const STALE: Reply = {
  status: 409,
  body: '{"code":"alert_stale","message":"告警状态已变化","data":null,"request_id":null}',
};

const UNAVAILABLE: Reply = {
  status: 503,
  body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
};

/** 当下可交互节点、读取序列及业务写请求都可直接观察。 */
export interface AlertsPageFixture {
  readonly tree: TestElement;
  readonly document: Parameters<Parameters<typeof withT11AdminPage>[2]>[0]["document"];
  readonly reads: string[];
  readonly writes: RecordedRequest[];
  readonly requests: readonly RecordedRequest[];
  recur(): void;
  navigate(): Promise<void>;
  filter(status: string): Promise<void>;
  reason(): TestElement;
  rowActions(disabled: boolean): readonly [TestElement, TestElement];
  setSilenceHours(hours: string): Promise<void>;
}

/** 如 Rust 的 AlertsPage 测试，在同一个外壳内装卸来源页。 */
export async function withAlertsPage(
  run: (page: AlertsPageFixture) => Promise<void>,
  respondToWrite?: (request: RecordedRequest) => Reply,
): Promise<void> {
  const { page: Page } = await loadRenderModule<typeof import("./index")>(
    new URL("./index.tsx", import.meta.url).pathname,
    "page",
  );
  const reads: string[] = [];
  const writes: RecordedRequest[] = [];
  let recurred = false;
  const fetch = installT11Fetch((request) => {
    if (request.url === "/health/ready") {
      expect(request.method).toBe("GET");
      return t11Reply({ ready: true });
    }
    if (request.method === "POST" && request.url.startsWith("/api/v1/browser-errors?")) {
      return t11Reply(null);
    }
    if (request.method === "POST") {
      writes.push(request);
      if (respondToWrite !== undefined) {
        return respondToWrite(request);
      }
      switch (writes.length) {
        case 1:
          return UNAVAILABLE;
        case 2:
          return STALE;
        case 3: {
          const match = /^\/api\/v1\/admin\/alerts\/([0-9a-f]+)\/(acknowledge|silence)$/u.exec(
            request.url,
          );
          if (match === null) {
            throw new Error(`告警写入路径不合法：${request.url}`);
          }
          const [, id, action] = match;
          return t11Reply({
            alert: {
              id,
              action,
              replayed: false,
              silenced_until: action === "silence" ? "2026-11-19T12:00:00Z" : null,
            },
          });
        }
        default:
          throw new Error(`未预期的告警写入：${request.url}`);
      }
    }

    const url = new URL(request.url, "https://auction.test");
    expect(request.method).toBe("GET");
    if (request.url === "/api/v1/admin/alerts/webhooks?limit=50") {
      return t11Reply({ entries: [] });
    }
    expect(url.pathname).toBe("/api/v1/admin/alerts");
    expect([...url.searchParams]).toEqual([
      ["status", expect.any(String)],
      ["page", "1"],
      ["page_size", "10"],
      ["include", "incident_generation"],
    ]);
    const status = url.searchParams.get("status");
    if (status === null) {
      throw new Error("告警读取没有状态筛选");
    }
    reads.push(status);
    const body = adminFixtureBody("alerts-list");
    const data = jsonObjectAt(body, "/data");
    const first = jsonObjectAt(body, "/data/alerts/0");
    if (recurred) {
      first.incident_generation = 3;
      first.count = 18;
    }
    data.page_size = Number(url.searchParams.get("page_size"));
    data.active_total = 1;
    data.total = status === "resolved" ? 0 : 1;
    data.alerts = status === "resolved" ? [] : [first];
    return { body: JSON.stringify(body) };
  });
  try {
    await withT11AdminPage("/alerts", Page, async ({ tree, document, navigate }) => {
      await run({
        tree,
        document,
        reads,
        writes,
        requests: fetch.requests,
        recur: () => {
          recurred = true;
        },
        async navigate() {
          navigate();
          await settleT11View();
        },
        async filter(status) {
          const control = oneT11Element(
            findAll(
              tree,
              (element) =>
                element.tag === "select" &&
                findAll(element, (option) => option.tag === "option" && option.value === "resolved")
                  .length === 1,
            ),
            "告警筛选",
          );
          fireT11Event(control, "onChange", status);
          await settleT11View();
        },
        reason() {
          const input = document.getElementById("alert-console-reason");
          if (input === null) {
            throw new Error(
              `没有挂载告警备注输入：${tree.textContent}；请求 ${JSON.stringify(fetch.requests)}`,
            );
          }
          return input;
        },
        rowActions(disabled) {
          const group = oneT11Element(
            findAll(
              tree,
              (element) => element.getAttribute("class") === "admin-alert-console__action-group",
            ),
            "告警行操作组",
          );
          expect(findAll(group, (element) => element.tag === "button")).toHaveLength(2);
          const acknowledge = oneT11Element(t11Buttons(group, "确认"), "行确认");
          const silence = oneT11Element(t11Buttons(group, "静默"), "行静默");
          expect(
            [acknowledge, silence].map((button) => button.attributes.disabled === true),
          ).toEqual([disabled, disabled]);
          return [acknowledge, silence];
        },
        async setSilenceHours(hours) {
          const control = oneT11Element(
            findAll(
              tree,
              (element) =>
                element.tag === "select" && element.getAttribute("aria-label") === "静默时长",
            ),
            "行静默时长",
          );
          fireT11Event(control, "onChange", hours);
          await settleT11View();
        },
      });
    });
  } finally {
    fetch.restore();
  }
}

/** 统计文本叶节点，避免父元素汇总同一消息导致重复计数。 */
export function textStartingWith(node: unknown, prefix: string): number {
  if (node instanceof TestElement) {
    return node.tag === "#text"
      ? Number(node.text.startsWith(prefix))
      : node.nodes.reduce((sum, child) => sum + textStartingWith(child(), prefix), 0);
  }
  if (typeof node === "function") {
    return textStartingWith(node(), prefix);
  }
  if (Array.isArray(node)) {
    return node.reduce<number>((sum, child) => sum + textStartingWith(child, prefix), 0);
  }
  return Number(typeof node === "string" && node.startsWith(prefix));
}

const REASON_ERROR = "备注过长，请缩短至 2048 个 UTF-8 字节以内。";

/** 分开观察字段错误和空挂载、延时填充的读屏播报区域。 */
export function expectReasonFeedback(
  tree: TestElement,
  visibleError: boolean,
  announced: boolean,
): void {
  const errors = findAll(tree, (element) => element.id === "alert-console-reason-error");
  expect(errors.map((element) => element.textContent)).toEqual(visibleError ? [REASON_ERROR] : []);
  const live = oneT11Element(reasonLiveRegions(tree), "告警备注播报区域");
  expect(live.getAttribute("aria-atomic")).toBe("true");
  expect(live.textContent).toBe(announced ? REASON_ERROR : "");
}

/** 只查备注的播报段落，供筛选卸载时检查其是否随备注离开。 */
export function reasonLiveRegions(tree: TestElement): TestElement[] {
  return findAll(
    tree,
    (element) => element.tag === "p" && element.getAttribute("aria-live") === "polite",
  );
}

test("告警夹具按文本叶节点计数，不把父元素汇总文本重复计入", () => {
  const tree = new TestElement("section");
  const child = new TestElement("p");
  insertNode(tree, child);
  insertNode(child, createTextNode("备注过长"));
  expect(textStartingWith(tree, "备注")).toBe(1);
  insertNode(child, createTextNode("备注已保存"));
  expect(textStartingWith(tree, "备注")).toBe(2);
});
