// 在同一次挂载中核对手动刷新、自动轮询与邮件轮询写回执。

import { expect, test } from "bun:test";
import { ok } from "@auction/web-shared/result";
import type { RecordedRequest } from "../../api/api-request-fixture";
import { parseJsonObject } from "../../api/api-request-fixture";
import { completePayload } from "../../api/dashboard/dashboard-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { findAll, type TestElement } from "../solid-dom-fixture";
import { installT11Fetch, t11Buttons, t11Reply, withT11AdminPage } from "./t11-page-fixture.test";
import {
  fireT11Event,
  installT11Clock,
  oneT11Element,
  settleT11View,
} from "./t11-solid-fixture.test";

const DASHBOARD_URL = "/api/v1/admin/dashboard?include=collection_wait";
const MAIL_URL = "/api/v1/admin/mail/polling";
const READY_URL = "/health/ready";

function urls(requests: readonly RecordedRequest[], url: string): string[] {
  return requests.filter((request) => request.url === url).map((request) => request.method);
}

function expectPollingReceipt(tree: TestElement, shown = true): void {
  const live = findAll(
    tree,
    (element) => element.tag === "div" && element.getAttribute("role") === "status",
  );
  const paragraphs = findAll(oneT11Element(live, "回执播报区域"), (element) => element.tag === "p");
  expect(paragraphs.map((element) => element.textContent)).toEqual(
    shown ? ["暂停定时轮询 (main)（mailbox:main）已成功。 轮询状态已更新。"] : [],
  );
}

function clickEnabledButton(tree: TestElement, label: string): void {
  const button = oneT11Element(t11Buttons(tree, label), label);
  expect(button.attributes.disabled).not.toBe(true);
  fireT11Event(button, "onClick");
}

test("manual_refresh_rechecks_the_shared_readiness_while_polling_only_reloads_the_snapshot", async () => {
  const { page: Page } = await loadRenderModule<typeof import("./index")>(
    new URL("./index.tsx", import.meta.url).pathname,
    "page",
  );
  let ready = true;
  const fetch = installT11Fetch((request) => {
    switch (request.url) {
      case READY_URL:
        return t11Reply({ ready });
      case DASHBOARD_URL:
        return t11Reply(completePayload());
      default:
        throw new Error(`意外请求：${request.url}`);
    }
  });
  try {
    const clock = await installT11Clock();
    try {
      await withT11AdminPage("/?poll=10", Page, async ({ tree, readiness }) => {
        expect(readiness()).toEqual(ok(true));
        expect(urls(fetch.requests, READY_URL)).toEqual(["GET"]);
        expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET"]);

        ready = false;
        for (let second = 0; second < 10; second += 1) {
          await clock.advance(1_000);
        }
        expect(readiness()).toEqual(ok(true));
        expect(urls(fetch.requests, READY_URL)).toEqual(["GET"]);
        expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET", "GET"]);

        clickEnabledButton(tree, "刷新快照");
        await settleT11View();
        expect(readiness()).toEqual(ok(false));
        expect(urls(fetch.requests, READY_URL)).toEqual(["GET", "GET"]);
        expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET", "GET", "GET"]);
      });
    } finally {
      clock.restore();
    }
  } finally {
    fetch.restore();
  }
});

test("dashboard_poll_receipt_survives_the_failed_aggregate_read", async () => {
  const { page: Page } = await loadRenderModule<typeof import("./index")>(
    new URL("./index.tsx", import.meta.url).pathname,
    "page",
  );
  const polling = {
    mailbox_key: "main",
    paused: false,
    state_version: 1,
    consecutive_failures: 0,
    last_poll_at: "2026-11-18T00:00:00Z",
    next_poll_at: "2026-11-18T00:01:00Z",
    updated_at: "2026-11-18T00:00:00Z",
  };
  const updated = { ...polling, paused: true, state_version: 2 };
  const dashboard = (state: typeof polling) =>
    t11Reply({
      ...completePayload(),
      mail: {
        status: "ok",
        observed_at: state.updated_at,
        data: { observed_at: state.updated_at, pollers: [state] },
      },
    });
  const unavailable = Promise.withResolvers<{ status: number; body: string }>();
  const unavailableReply = {
    status: 503,
    body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
  };
  const written = Promise.withResolvers<ReturnType<typeof t11Reply>>();
  const writeReply = t11Reply({ polling: updated });
  const reloaded = Promise.withResolvers<ReturnType<typeof t11Reply>>();
  const reloadedReply = dashboard(updated);
  const reads = [dashboard(polling), unavailable.promise, reloaded.promise];
  const fetch = installT11Fetch((request) => {
    switch (request.url) {
      case READY_URL:
        return t11Reply({ ready: true });
      case MAIL_URL:
        return written.promise;
      case DASHBOARD_URL: {
        const reply = reads.shift();
        if (reply === undefined) throw new Error("未预期的聚合读取");
        return reply;
      }
      default:
        throw new Error(`意外请求：${request.url}`);
    }
  });
  try {
    await withT11AdminPage("/?poll=0", Page, async ({ tree }) => {
      const pause = "暂停定时轮询 (main)";
      const resume = "恢复定时轮询 (main)";
      clickEnabledButton(tree, pause);
      await settleT11View();
      expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET"]);
      expect(urls(fetch.requests, MAIL_URL)).toEqual(["PUT"]);
      const write = fetch.requests.find((request) => request.url === MAIL_URL);
      if (write === undefined) throw new Error("没有记录到轮询写入");
      expect(parseJsonObject(write.body, "邮件轮询请求")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u),
        mailbox_key: "main",
        expected_state_version: 1,
        paused: true,
      });
      expectPollingReceipt(tree, false);

      written.resolve(writeReply);
      await settleT11View();
      expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET", "GET"]);
      expectPollingReceipt(tree);

      unavailable.resolve(unavailableReply);
      await settleT11View();
      expect(t11Buttons(tree, pause)).toEqual([]);
      const error = oneT11Element(
        findAll(
          tree,
          (element) => element.tag === "section" && element.getAttribute("role") === "alert",
        ),
        "聚合读取失败",
      );
      expect(error.querySelector("h2")?.textContent).toBe("所需服务暂时不可用。请稍后再试。");
      expectPollingReceipt(tree);
      expect(t11Buttons(tree, "关闭回执")).toHaveLength(1);

      clickEnabledButton(error, "重试");
      await settleT11View();
      expect(urls(fetch.requests, DASHBOARD_URL)).toEqual(["GET", "GET", "GET"]);
      expect(urls(fetch.requests, MAIL_URL)).toEqual(["PUT"]);
      reloaded.resolve(reloadedReply);
      await settleT11View();
      expect(t11Buttons(tree, resume)).toHaveLength(1);
      expectPollingReceipt(tree);
    });
  } finally {
    try {
      // 即使用例提前失败，也让已发出的写入和聚合读取离开等待并完成传输层清理。
      written.resolve(writeReply);
      unavailable.resolve(unavailableReply);
      reloaded.resolve(reloadedReply);
      await settleT11View();
    } finally {
      fetch.restore();
    }
  }
});
