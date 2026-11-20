// 仪表盘邮件操作的写回执、读取失效与未决命令。

import { expect, test } from "bun:test";
import { parseJsonObject, type Reply, TEST_SESSION_BODY } from "../../api/api-request-fixture";
import { completePayload } from "../../api/dashboard/dashboard-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { findAll, type TestElement } from "../solid-dom-fixture";
import { installT11Fetch, t11Buttons, t11Reply, withT11AdminPage } from "./t11-page-fixture.test";
import { fireT11Event, oneT11Element, settleT11View } from "./t11-solid-fixture.test";

const DASHBOARD = "/api/v1/admin/dashboard?include=collection_wait";
const SESSION = "/api/v1/admin/session?include=server_time";
const TRIGGER = "/api/v1/admin/mail/polls:trigger";
const POLLING = "/api/v1/admin/mail/polling";
const COUNT = "已向 3 个轮询器请求执行轮询。";
const POLL = "手动触发收件轮询";
const PAUSE = "暂停定时轮询 (main)";
const RESUME = "恢复定时轮询 (main)";
const BROKEN = { status: 409, body: '{"code":' };

function dashboard(paused = false): Reply {
  const observed_at = "2026-11-18T00:00:00Z";
  return t11Reply({
    ...completePayload(),
    mail: {
      status: "ok",
      observed_at,
      data: {
        observed_at,
        pollers: [
          {
            mailbox_key: "main",
            paused,
            state_version: 1,
            consecutive_failures: 0,
            last_poll_at: observed_at,
            next_poll_at: "2026-11-18T00:01:00Z",
            updated_at: observed_at,
          },
        ],
      },
    },
  });
}

async function pageModule() {
  return (
    await loadRenderModule<typeof import("./index")>(
      new URL("./index.tsx", import.meta.url).pathname,
      "page",
    )
  ).page;
}

async function click(tree: TestElement, label: string): Promise<void> {
  const button = oneT11Element(t11Buttons(tree, label), label);
  expect(button.attributes.disabled).not.toBe(true);
  fireT11Event(button, "onClick");
  await settleT11View();
}

async function retry(tree: TestElement): Promise<void> {
  const panel = oneT11Element(
    tree
      .querySelectorAll("section")
      .filter((node) => node.getAttribute("aria-label") === "未决操作"),
    "未决操作",
  );
  await click(panel, "原样重试");
}

test.each(["匹配", "错误状态", "错误命令 ID", "写入 503"] as const)(
  "dashboard_poll_trigger_delivers_only_matching_202_receipts (%s)",
  async (variant) => {
    const Page = await pageModule();
    let writes = 0;
    const fetch = installT11Fetch((request) => {
      if (request.url === "/health/ready") return t11Reply({ ready: true });
      if (request.url === DASHBOARD) return dashboard();
      if (request.url !== TRIGGER) throw new Error(`意外请求：${request.url}`);
      writes++;
      const command = parseJsonObject(request.body, "立即轮询请求");
      if (writes === 1 && variant === "写入 503") {
        return {
          status: 503,
          body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
        };
      }
      return {
        status: writes === 1 && variant === "错误状态" ? 200 : 202,
        body: JSON.stringify({
          code: "accepted",
          message: "邮件轮询已触发",
          data: {
            command_id:
              writes === 1 && variant === "错误命令 ID"
                ? "00000000-0000-4000-8000-000000000029"
                : command.command_id,
            pollers: 3,
          },
        }),
      };
    });
    try {
      await withT11AdminPage("/?poll=0", Page, async ({ tree }) => {
        await click(tree, POLL);
        const original = fetch.requests.find((request) => request.url === TRIGGER);
        if (original === undefined) throw new Error("缺少立即轮询 POST");
        expect(original.method).toBe("POST");
        expect(parseJsonObject(original.body, "立即轮询请求")).toEqual({
          command_id: expect.stringMatching(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u),
        });
        if (variant !== "匹配") {
          expect(tree.textContent).not.toContain(COUNT);
          expect(t11Buttons(tree, "关闭回执")).toHaveLength(0);
          expect(oneT11Element(t11Buttons(tree, POLL), POLL).attributes.disabled).toBe(true);
          expect(fetch.requests.filter(({ url }) => url === DASHBOARD)).toHaveLength(1);
          await retry(tree);
        }
        const live = findAll(
          tree,
          (node) => node.tag === "div" && node.getAttribute("role") === "status",
        );
        const notices = findAll(oneT11Element(live, "回执播报区域"), (node) => node.tag === "p");
        expect(notices.map((node) => node.textContent)).toEqual([
          `${POLL}（mail-poll-trigger）已成功。 ${COUNT}`,
        ]);
        expect(t11Buttons(tree, "关闭回执")).toHaveLength(1);
        expect(t11Buttons(tree, "原样重试")).toHaveLength(0);
        const commandRequests = fetch.requests.filter(
          ({ url }) => url === SESSION || url === TRIGGER,
        );
        const csrf = { url: SESSION, method: "GET", body: "" };
        expect(commandRequests).toEqual(
          variant === "匹配" ? [csrf, original] : [csrf, original, csrf, original],
        );
      });
    } finally {
      fetch.restore();
    }
  },
);

test.each(["完整坏正文", "完整坏正文 503", "写前 GET", "写响应截断"] as const)(
  "dashboard_pause_resume_refresh_only_for_a_complete_write_409 (%s)",
  async (variant) => {
    const Page = await pageModule();
    const paused = variant !== "完整坏正文";
    let reviews = 0;
    const fetch = installT11Fetch(
      (request) => {
        switch (request.url) {
          case "/health/ready":
            return t11Reply({ ready: true });
          case DASHBOARD:
            return dashboard(paused);
          case POLLING:
            if (variant === "完整坏正文 503") return { ...BROKEN, status: 503 };
            if (variant === "写前 GET" || variant === "写响应截断") {
              return new Response(
                new ReadableStream<Uint8Array>({
                  start(controller) {
                    controller.enqueue(new TextEncoder().encode('{"code":'));
                    controller.error(new Error("响应流截断"));
                  },
                }),
                { status: 409 },
              );
            }
            return BROKEN;
          default:
            throw new Error(`意外请求：${request.url}`);
        }
      },
      variant === "写前 GET"
        ? () => {
            reviews++;
            return reviews === 1
              ? { body: JSON.stringify(TEST_SESSION_BODY) }
              : {
                  status: 409,
                  body: '{"code":"conflict","message":"会话复核失败","data":null,"request_id":null}',
                };
          }
        : undefined,
    );
    try {
      await withT11AdminPage("/?poll=0", Page, async ({ tree }) => {
        await click(tree, paused ? RESUME : PAUSE);
        if (variant === "写前 GET") {
          await retry(tree);
        }
        const write = fetch.requests.filter(({ url }) => url === POLLING);
        expect(
          fetch.requests.filter(({ url }) => url === SESSION).map(({ method }) => method),
        ).toEqual(variant === "写前 GET" ? ["GET", "GET"] : ["GET"]);
        expect(write).toHaveLength(1);
        if (write[0] !== undefined) {
          expect(write[0].method).toBe("PUT");
          expect(parseJsonObject(write[0].body, "暂停或恢复请求")).toEqual({
            command_id: expect.stringMatching(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u),
            mailbox_key: "main",
            expected_state_version: 1,
            paused: !paused,
          });
        }
        expect(
          fetch.requests.filter(({ url }) => url === DASHBOARD).map(({ method }) => method),
        ).toEqual(variant === "完整坏正文" ? ["GET", "GET"] : ["GET"]);
        expect(t11Buttons(tree, "关闭回执")).toHaveLength(0);
        expect(t11Buttons(tree, "原样重试")).toHaveLength(2);
        const action = paused ? RESUME : PAUSE;
        expect(oneT11Element(t11Buttons(tree, action), action).attributes.disabled).toBe(true);
        expect(tree.textContent).not.toContain("轮询状态已更新。");
      });
    } finally {
      fetch.restore();
    }
  },
);
