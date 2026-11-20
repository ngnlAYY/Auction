// T1 的未决命令测试宿主，从本地请求回执观察旧历史及重试身份。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { err } from "@auction/web-shared/result";
import { createRoot } from "solid-js";

import {
  type CommandNotice,
  createPendingCommands,
  type PendingCommandHost,
  type PendingCommands,
  usePendingCommandHost,
} from "../surface/pending";
import type { CommandSource } from "../surface/pending/sources";
import { installBrowserRequest } from "./api-request-fixture";
import { type AdminApiFailure, adminApiFailureDisplayCode } from "./failure";
import { prepareMailTemplateAttemptOnce, submitMailTemplate } from "./notification-templates";
import { type AdminSession, confirmedAdminSession, registerAdminSessionHost } from "./session";
import { spyOnBrowserFetch } from "./t1-request-fixture.test";

const SESSION: AdminSession = {
  actor: "operator",
  csrfToken: "csrf-token",
  csrf_token: "csrf-token",
  expiresAt: 0n,
  absoluteExpiresAt: 0n,
  serverTime: 0n,
  receivedAtMillis: 0,
};
const SESSION_BODY = {
  code: "ok",
  message: "",
  data: {
    authenticated: true,
    actor: SESSION.actor,
    csrf_token: SESSION.csrfToken,
    expires_at: "2026-11-19T01:00:00Z",
    absolute_expires_at: "2026-11-20T00:00:00Z",
    server_time: "2026-11-19T00:00:00Z",
  },
};
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const TEMPLATE_KEY = "settlement_seller_payout";
const WRITE_URL = "/api/v1/admin/mail-templates/settlement_seller_payout";

async function waitFor(condition: () => boolean, label: string): Promise<void> {
  const deadline = Date.now() + 1_000;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error(`等待超时：${label}`);
    }
    await Bun.sleep(0);
  }
}

async function withPendingCommands<T>(
  session: AdminSession | null,
  run: (host: PendingCommandHost, commands: PendingCommands, checks: string[]) => Promise<T>,
): Promise<T> {
  const checks: string[] = [];
  let dispose = (): void => {};
  const registration = registerAdminSessionHost({
    check: () => {
      checks.push("check");
    },
    writesAllowed: () => true,
    confirmed: () => session,
  });
  try {
    const host = createRoot((disposeRoot) => {
      dispose = disposeRoot;
      return usePendingCommandHost(registration.host);
    });
    const source = createCell<CommandSource>({
      reload: createCell(0n),
      gate: null,
      rejections: createCell<readonly CommandNotice[]>([]),
      matchesEntity: () => true,
    });
    host.sources.set([new WeakRef(source.peek())]);
    return await run(host, createPendingCommands(host, source), checks);
  } finally {
    try {
      dispose();
    } finally {
      registration.release();
    }
  }
}

/** 建立一次未知结果，再用给定稳定码重试；返回是否按 Rust 规则清除了旧历史。 */
export async function runHistoryCase(code: string, clears: boolean): Promise<void> {
  await withPendingCommands(null, async (hostRef, commands) => {
    const ids: string[] = [];
    let attempt = 0;
    expect(
      commands.submitWithReceipt(
        "mail-template:1",
        "save",
        async (commandId) => {
          attempt += 1;
          ids.push(commandId);
          // 无 HTTP 状态的普通失败没有拒绝证据，先保留未知历史。
          return attempt === 1 ? err("network_error") : err(code);
        },
        () => ({ kind: "delivered" }),
        () => {},
      ),
    ).toBe(true);
    await waitFor(() => hostRef.entries.peek()[0]?.unknown === true, "首次未知结果");
    const commandId = hostRef.entries.peek()[0]?.identity.commandId;
    if (commandId === undefined) throw new Error("未登记首次命令");

    hostRef.retry(commandId);
    if (clears) {
      await waitFor(() => attempt === 2 && hostRef.entries.peek().length === 0, "重放检查后清除");
    } else {
      await waitFor(() => attempt === 2 && hostRef.entries.peek()[0]?.busy === false, "保留旧历史");
      const entry = hostRef.entries.peek()[0];
      expect(entry?.unknown, code).toBe(true);
      expect(entry?.error, code).toBe(adminApiFailureDisplayCode(code));
    }
    // 重试必须复用首次生成的命令 ID，不能用新 ID 改写未知历史。
    expect(ids, code).toEqual([commandId, commandId]);
  });
}

interface HistoryRetry {
  readonly target: "session" | "write";
  readonly status: number;
  readonly body: string;
  readonly display: string;
  readonly clears: boolean;
}

/** 让同一模板命令先失去回执，再通过真实 CSRF 复核和模板写入入口重试。 */
export async function runMailTemplateHistory(
  label: string,
  retries: readonly HistoryRetry[],
): Promise<void> {
  const restoreRequest = installBrowserRequest();
  try {
    const fetch = spyOnBrowserFetch();
    try {
      await withPendingCommands(SESSION, async (host, commands, checks) => {
        let reply: HistoryRetry | null = null;
        const requests: Array<readonly [string, string]> = [];
        const writeBodies: string[] = [];
        fetch.respondWith(async (input, init) => {
          if (typeof input !== "string") throw new Error("模板命令应发送相对 URL");
          const method = init?.method ?? "GET";
          requests.push([method, input]);
          if (input === SESSION_URL && method === "GET") {
            return reply?.target === "session"
              ? new Response(reply.body, { status: reply.status })
              : Response.json(SESSION_BODY);
          }
          if (input === WRITE_URL && method === "PUT" && typeof init?.body === "string") {
            writeBodies.push(init.body);
            if (reply?.target === "write")
              return new Response(reply.body, { status: reply.status });
          }
          // 首次写入已交给 fetch，连接断开后客户端没有回执。
          throw new TypeError("未收到写回执");
        });

        const ids: string[] = [];
        const failures: Array<AdminApiFailure | null> = [];
        let prepare: ReturnType<typeof prepareMailTemplateAttemptOnce> | null = null;
        expect(
          commands.submit(
            `mail-template:${TEMPLATE_KEY}`,
            "save",
            async (commandId, signal) => {
              ids.push(commandId);
              prepare ??= prepareMailTemplateAttemptOnce({
                commandId,
                key: TEMPLATE_KEY,
                expectedRevision: 1,
                body: null,
              });
              const prepared = prepare(commandId);
              const result = prepared.ok
                ? await submitMailTemplate(host.sessionHost, prepared.value, signal)
                : prepared;
              failures.push(result.ok ? null : result.error);
              return result.ok ? result : err(result.error.code);
            },
            () => {},
            () => {},
          ),
          label,
        ).toBe(true);
        await waitFor(() => host.entries.peek()[0]?.unknown === true, label);
        expect(host.entries.peek(), label).toHaveLength(1);
        const initial = host.entries.peek()[0];
        if (initial === undefined) throw new Error("未登记首次命令");
        const { commandId } = initial.identity;
        expect(failures, label).toEqual([{ status: null, code: "network_error" }]);
        expect(initial.error, label).toBe("network_error");
        expect(ids, label).toEqual([commandId]);
        expect(requests, label).toEqual([
          ["GET", SESSION_URL],
          ["PUT", WRITE_URL],
        ]);
        expect(checks, label).toHaveLength(0);
        expect(writeBodies, label).toHaveLength(1);
        const initialBody = writeBodies[0];
        if (initialBody === undefined) throw new Error("未记录首次写入正文");
        expect(parseJson(initialBody), label).toEqual({
          ok: true,
          value: { command_id: commandId, expected_revision: 1, body: null },
        });

        for (const [index, retry] of retries.entries()) {
          reply = retry;
          const step = `${label} ${retry.target} ${retry.status}`;
          const beforeRequests = requests.length;
          const beforeWrites = writeBodies.length;
          const beforeChecks = checks.length;
          host.retry(commandId);
          await waitFor(
            () => failures.length === index + 2 && !host.entries.peek().some((entry) => entry.busy),
            step,
          );
          const preflight = retry.target === "session";
          expect(requests.slice(beforeRequests), step).toEqual(
            preflight
              ? [["GET", SESSION_URL]]
              : [
                  ["GET", SESSION_URL],
                  ["PUT", WRITE_URL],
                ],
          );
          expect(writeBodies.slice(beforeWrites), step).toEqual(preflight ? [] : [initialBody]);
          expect(checks.length - beforeChecks, step).toBe(preflight ? 1 : 0);
          expect(failures[index + 1], step).toEqual({
            status: preflight ? null : retry.status,
            code: preflight
              ? `not_sent:${retry.display}`
              : `write_endpoint:mail-template:${retry.status}:${retry.display}`,
          });
          expect(ids, step).toEqual(Array.from({ length: index + 2 }, () => commandId));
          const entries = host.entries.peek();
          expect(entries, step).toHaveLength(retry.clears ? 0 : 1);
          if (!retry.clears) {
            expect(entries[0]?.identity, step).toEqual(initial.identity);
            expect(entries[0]?.unknown, step).toBe(true);
            expect(entries[0]?.error, step).toBe(retry.display);
          }
        }
      });
    } finally {
      fetch.restore();
    }
  } finally {
    restoreRequest();
  }
}

test("未决历史夹具在检查结束后释放会话宿主", async () => {
  const session = await withPendingCommands(SESSION, async (host) => {
    expect(confirmedAdminSession(host.sessionHost)).toBe(SESSION);
    return host.sessionHost;
  });
  expect(confirmedAdminSession(session)).toBeNull();
});
