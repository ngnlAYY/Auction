// 重放弹窗在失败列表更新时保留草稿，已登记命令交由会话继续核对。

import { expect, test } from "bun:test";
import {
  jsonObjectAt,
  parseJsonObject,
  type RecordedRequest,
  type Reply,
} from "../../api/api-request-fixture";
import { adminFixtureBody } from "../../api/t11-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  installT11Fetch,
  t11Buttons,
  t11Reply,
  withT11AdminPage,
} from "../dashboard/t11-page-fixture.test";
import {
  fireT11Event,
  installT11Clock,
  oneT11Element,
  settleT11View,
} from "../dashboard/t11-solid-fixture.test";
import { findAll, type TestElement } from "../solid-dom-fixture";

const LIST = "/api/v1/admin/realtime/failures?page=2&page_size=10&consumer_key=public-realtime";
const COUNT = "/api/v1/admin/realtime/failures?page=1&page_size=10&resolved=false";
const WRITE = "/api/v1/admin/realtime/failures/00000000-0000-4000-8000-000000000027/replay";
const PROCESSED = "该记录已被处理，或已不在当前列表中。";
const UNAVAILABLE = "所需服务暂时不可用。请稍后再试。";
const SUCCESS = "重放已完成。";
const PAYLOAD = '{"checked":true}';
const READ_ERROR = {
  status: 503,
  body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
};
const OTHER_EVENT = "00000000-0000-4000-8000-000000000029";

function listReply(replacement: object | null = {}): Reply {
  const body = adminFixtureBody("realtime-failures");
  const first = jsonObjectAt(body, "/data/failures/0");
  return t11Reply({
    failures: replacement === null ? [] : [{ ...first, ...replacement }],
    pagination: { page: 2, page_size: 10, total: 11, has_more: false },
  });
}

function receipt(original?: string): Reply {
  const event = jsonObjectAt(adminFixtureBody("realtime-events"), "/data/events/0");
  return t11Reply({
    replay: {
      ...event,
      event_id: "00000000-0000-4000-8000-000000000028",
      replay_of_event_id: original ?? event.event_id,
      schema_version: 3,
    },
  });
}

function button(tree: TestElement, label: string): TestElement {
  return oneT11Element(t11Buttons(tree, label), label);
}

async function click(tree: TestElement, label: string): Promise<void> {
  const target = button(tree, label);
  expect(target.attributes.disabled).not.toBe(true);
  fireT11Event(target, "onClick");
  await settleT11View();
}

function dialog(tree: TestElement): TestElement {
  return oneT11Element(
    findAll(tree, (element) => element.getAttribute("class") === "admin-realtime-dialog"),
    "重放弹窗",
  );
}

function draftControls(tree: TestElement): [TestElement, TestElement] {
  const modal = dialog(tree);
  return [
    oneT11Element(modal.querySelectorAll("input"), "Schema 版本"),
    oneT11Element(modal.querySelectorAll("textarea"), "公开负载"),
  ];
}

async function openDraft(tree: TestElement): Promise<void> {
  await click(tree, "重放");
  const [schema, payload] = draftControls(tree);
  fireT11Event(schema, "onInput", "3");
  fireT11Event(payload, "onInput", PAYLOAD);
  await settleT11View();
}

function expectDraft(tree: TestElement): void {
  expect(draftControls(tree).map((control) => control.value)).toEqual(["3", PAYLOAD]);
}

function requestsAt(requests: readonly RecordedRequest[], url: string): RecordedRequest[] {
  return requests.filter((request) => request.url === url);
}

function expectReplayBody(request: RecordedRequest | undefined): void {
  expect(request?.method).toBe("POST");
  expect(parseJsonObject(request?.body ?? "", "重放请求")).toEqual({
    command_id: expect.stringMatching(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u),
    schema_version: 3,
    public_payload: { checked: true },
  });
}

async function withReplayPage(
  nextList: () => Reply | Promise<Reply>,
  write: () => Reply | Promise<Reply>,
  run: (
    tree: TestElement,
    requests: readonly RecordedRequest[],
    tick: () => Promise<void>,
  ) => Promise<void>,
): Promise<void> {
  const { outboxPage } = await loadRenderModule<typeof import("./page")>(
    new URL("./page.tsx", import.meta.url).pathname,
    "outboxPage",
  );
  const fetch = installT11Fetch((request) => {
    if (request.url === "/health/ready") return t11Reply({ ready: true });
    if (request.url === LIST) return nextList();
    if (request.url === WRITE) return write();
    if (request.method === "POST" && request.url.startsWith("/api/v1/browser-errors?")) {
      return t11Reply(null);
    }
    const url = new URL(request.url, "https://auction.test");
    const page = Number(url.searchParams.get("page"));
    const page_size = Number(url.searchParams.get("page_size"));
    const pagination = { page, page_size, total: 0 };
    switch (url.pathname) {
      case "/api/v1/admin/realtime/events":
        return t11Reply({ events: [], pagination });
      case "/api/v1/admin/realtime/lag":
        return t11Reply({ lags: [], pagination });
      case "/api/v1/admin/realtime/failures":
        expect(request.url).toBe(COUNT);
        return t11Reply({ failures: [], pagination: { ...pagination, has_more: false } });
      default:
        throw new Error(`意外请求：${request.url}`);
    }
  });
  try {
    const clock = await installT11Clock();
    try {
      await withT11AdminPage(
        "/outbox?failure_page=2&failure_consumer=public-realtime&poll=10",
        outboxPage,
        async ({ tree }) =>
          run(tree, fetch.requests, async () => {
            for (let second = 0; second < 10; second += 1) await clock.advance(1_000);
          }),
      );
    } finally {
      clock.restore();
    }
  } finally {
    fetch.restore();
  }
}

function stopPolling(tree: TestElement): void {
  const select = oneT11Element(
    tree
      .querySelectorAll("select")
      .filter((control) =>
        control.querySelectorAll("option").some((option) => option.value === "60"),
      ),
    "自动轮询",
  );
  fireT11Event(select, "onChange", "0");
}

test.each([
  ["已解决", { resolved: true, resolved_at: "2025-11-15T10:00:00Z" }],
  ["离开列表", null],
  ["原事件变更", { event_id: OTHER_EVENT }],
  ["流键变更", { stream_key: "auction:other" }],
] as const)("realtime_replay_stays_disabled_until_reopened (%s)", async (_label, replacement) => {
  let snapshot = listReply();
  await withReplayPage(
    () => snapshot,
    receipt,
    async (tree, requests, tick) => {
      await openDraft(tree);
      await tick();
      expectDraft(tree);
      expect(button(dialog(tree), "确认重放").attributes.disabled).toBe(false);
      snapshot = listReply(replacement);
      await tick();
      expect(dialog(tree).textContent).toContain(PROCESSED);
      expectDraft(tree);
      let submit = button(dialog(tree), "确认重放");
      expect(submit.attributes.disabled).toBe(true);
      fireT11Event(submit, "onClick");
      await settleT11View();
      expect(requestsAt(requests, WRITE)).toEqual([]);

      snapshot = listReply();
      await tick();
      expect(dialog(tree).textContent).toContain(PROCESSED);
      submit = button(dialog(tree), "确认重放");
      expect(submit.attributes.disabled).toBe(true);
      fireT11Event(submit, "onClick");
      await settleT11View();
      expect(requestsAt(requests, WRITE)).toEqual([]);
      await click(dialog(tree), "取消");

      await openDraft(tree);
      await click(dialog(tree), "确认重放");
      const writes = requestsAt(requests, WRITE);
      expect(writes).toHaveLength(1);
      expectReplayBody(writes[0]);
      expect(tree.textContent.split(SUCCESS)).toHaveLength(2);
    },
  );
});

test("realtime_replay_retries_only_failure_reads_and_settles_after_an_inflight_resolution", async () => {
  let snapshot: Reply | Promise<Reply> = listReply();
  const reread = Promise.withResolvers<Reply>();
  const write = Promise.withResolvers<Reply>();
  try {
    await withReplayPage(
      () => snapshot,
      () => write.promise,
      async (tree, requests, tick) => {
        await openDraft(tree);
        snapshot = READ_ERROR;
        await tick();
        stopPolling(tree);
        await settleT11View();
        expect(dialog(tree).textContent).toContain(UNAVAILABLE);
        expect(dialog(tree).textContent).not.toContain(PROCESSED);
        expectDraft(tree);
        const failedSubmit = button(dialog(tree), "确认重放");
        expect(failedSubmit.attributes.disabled).toBe(true);
        fireT11Event(failedSubmit, "onClick");
        await settleT11View();
        expect(requestsAt(requests, WRITE)).toEqual([]);

        snapshot = reread.promise;
        const requestOffset = requests.length;
        await click(dialog(tree), "重试");
        const busyRetry = button(dialog(tree), "重试");
        expect(busyRetry.attributes.disabled).toBe(true);
        fireT11Event(busyRetry, "onClick");
        await settleT11View();
        expect(
          requests
            .slice(requestOffset)
            .map(({ method, url }) => [method, url])
            .sort(),
        ).toEqual(
          [
            ["GET", COUNT],
            ["GET", LIST],
          ].sort(),
        );
        reread.resolve(READ_ERROR);
        await settleT11View();
        expect(dialog(tree).textContent).toContain(UNAVAILABLE);
        expect(dialog(tree).textContent).not.toContain(PROCESSED);
        expectDraft(tree);
        expect(button(dialog(tree), "确认重放").attributes.disabled).toBe(true);

        const secondOffset = requests.length;
        snapshot = listReply();
        await click(dialog(tree), "重试");
        expect(
          requests
            .slice(secondOffset)
            .map(({ method, url }) => [method, url])
            .sort(),
        ).toEqual(
          [
            ["GET", COUNT],
            ["GET", LIST],
          ].sort(),
        );
        expect(dialog(tree).textContent).not.toContain(UNAVAILABLE);
        expectDraft(tree);
        await click(dialog(tree), "确认重放");
        const writes = requestsAt(requests, WRITE);
        expect(writes).toHaveLength(1);
        expectReplayBody(writes[0]);
        expect(tree.textContent).not.toContain(SUCCESS);

        const pollControl = oneT11Element(
          tree
            .querySelectorAll("select")
            .filter((control) =>
              control.querySelectorAll("option").some((option) => option.value === "60"),
            ),
          "自动轮询",
        );
        fireT11Event(pollControl, "onChange", "10");
        await settleT11View();
        snapshot = listReply({ resolved: true, resolved_at: "2025-11-15T10:00:00Z" });
        await tick();
        expect(t11Buttons(tree, "重放")).toHaveLength(0);
        write.resolve(receipt());
        await settleT11View();
        expect(tree.textContent.split(SUCCESS)).toHaveLength(2);
        expect(requestsAt(requests, WRITE)).toHaveLength(1);
      },
    );
  } finally {
    reread.resolve(READ_ERROR);
    write.resolve(receipt());
    await settleT11View();
  }
});

test("realtime_replay_retries_the_frozen_request_after_the_list_changes", async () => {
  let snapshot = listReply();
  let reply = receipt(OTHER_EVENT);
  await withReplayPage(
    () => snapshot,
    () => reply,
    async (tree, requests, tick) => {
      await openDraft(tree);
      await click(dialog(tree), "确认重放");
      const [original] = requestsAt(requests, WRITE);
      if (original === undefined) throw new Error("缺少首个重放请求");
      expectReplayBody(original);
      expect(tree.textContent).not.toContain(SUCCESS);
      const panel = oneT11Element(
        tree
          .querySelectorAll("section")
          .filter((node) => node.getAttribute("aria-label") === "未决操作"),
        "重放未决操作",
      );
      expect(t11Buttons(panel, "原样重试")).toHaveLength(1);
      snapshot = listReply({ event_id: OTHER_EVENT, stream_key: "auction:other" });
      await tick();
      reply = receipt();
      await click(panel, "原样重试");
      expect(requestsAt(requests, WRITE)).toEqual([original, original]);
      expect(tree.textContent.split(SUCCESS)).toHaveLength(2);
      expect(t11Buttons(tree, "原样重试")).toHaveLength(0);
    },
  );
});

test("realtime_replay_preserves_an_invalid_json_draft_without_registering_a_command", async () => {
  await withReplayPage(listReply, receipt, async (tree, requests) => {
    await openDraft(tree);
    const [, payload] = draftControls(tree);
    fireT11Event(payload, "onInput", "{");
    await settleT11View();
    const requestOffset = requests.length;

    await click(dialog(tree), "确认重放");

    expect(requests.slice(requestOffset).filter(({ method }) => method === "POST")).toEqual([]);
    expect(
      findAll(
        dialog(tree),
        (node) => node.getAttribute("role") === "alert" && node.textContent === "重放修正无效。",
      ),
    ).toHaveLength(1);
    expect(draftControls(tree).map((control) => control.value)).toEqual(["3", "{"]);
    expect(findAll(tree, (node) => node.getAttribute("aria-label") === "未决操作")).toHaveLength(0);
  });
});

test("realtime_replay_reuses_its_command_id_until_a_verified_conflict_settles_it", async () => {
  let reply = READ_ERROR;
  await withReplayPage(
    listReply,
    () => reply,
    async (tree, requests) => {
      await openDraft(tree);
      await click(dialog(tree), "确认重放");
      const [original] = requestsAt(requests, WRITE);
      if (original === undefined) throw new Error("缺少首个重放请求");
      expectReplayBody(original);
      const panel = oneT11Element(
        tree
          .querySelectorAll("section")
          .filter((node) => node.getAttribute("aria-label") === "未决操作"),
        "重放未决操作",
      );
      const blocked = button(tree, "重放");
      expect(blocked.attributes.disabled).toBe(true);
      fireT11Event(blocked, "onClick");
      await settleT11View();
      expect(t11Buttons(tree, "确认重放")).toHaveLength(0);
      reply = {
        status: 409,
        body: '{"code":"conflict","message":"命令冲突","data":null,"request_id":null}',
      };
      await click(panel, "原样重试");
      expect(requestsAt(requests, WRITE)).toEqual([original, original]);
      expect(tree.textContent).not.toContain(SUCCESS);
      expect(t11Buttons(tree, "关闭已拒绝操作")).toHaveLength(1);
      await click(tree, "重放");
      expect(t11Buttons(tree, "确认重放")).toHaveLength(1);
    },
  );
});
