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
import { createPendingHarness, waitForAttempt } from "../surface/pending/pending-command-fixture";
import type { CommandSource } from "../surface/pending/sources";
import { installBrowserRequest } from "./api-request-fixture";
import {
  type AdminWriteFailure,
  adminWriteFailureDisplayCode,
  observedResponseStatus,
  writeOutcomeUnknown,
} from "./failure";
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
const SETTLED_BY_HISTORY = {
  unknown: [false, false],
  this_attempt: [true, false],
  replay_checked: [true, true],
} as const;

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

/** 用同一份失败分别裁定首次尝试和已有未知结果的重试。 */
export async function runHistoryCase(
  failure: AdminWriteFailure,
  expected: {
    readonly display: string;
    readonly scope: keyof typeof SETTLED_BY_HISTORY;
  },
): Promise<void> {
  for (const [index, settled] of SETTLED_BY_HISTORY[expected.scope].entries()) {
    const earlierUnknown = index === 1;
    const harness = createPendingHarness();
    try {
      const { host, origin } = harness;
      const entity = "mail-template:1";
      const ids: string[] = [];
      const displayed: string[] = [];
      const label = `${expected.display}（${earlierUnknown ? "已有未知" : "首次尝试"}）`;
      expect(
        origin.commands.submit(
          entity,
          "save",
          async (commandId) => {
            ids.push(commandId);
            return earlierUnknown && ids.length === 1
              ? err(writeOutcomeUnknown("request_timeout"))
              : err(failure);
          },
          () => {
            throw new Error("失败不能交付成功回执");
          },
          (code) => displayed.push(code),
        ),
        label,
      ).toBe(true);
      await waitForAttempt(() => origin.commands.isBusy(entity));
      const commandId = ids[0];
      if (commandId === undefined) throw new Error("未登记首次命令");

      if (earlierUnknown) {
        expect(host.entries.peek()[0], label).toMatchObject({
          identity: { commandId },
          unknown: true,
          error: "request_timeout",
        });
        expect(origin.rejections.peek(), label).toEqual([]);
        expect(origin.reload.peek(), label).toBe(0n);
        host.retry(commandId);
        await waitForAttempt(() => origin.commands.isBusy(entity));
      }

      expect(origin.commands.contains(entity), label).toBe(!settled);
      expect(host.entries.peek(), label).toHaveLength(settled ? 0 : 1);
      expect(origin.reload.peek(), label).toBe(settled ? 1n : 0n);
      expect(host.notices.peek(), label).toEqual([]);
      if (settled) {
        expect(origin.rejections.peek(), label).toHaveLength(1);
        expect(origin.rejections.peek()[0], label).toMatchObject({
          identity: { commandId },
          result: err(expected.display),
        });
      } else {
        expect(origin.rejections.peek(), label).toEqual([]);
        expect(host.entries.peek()[0], label).toMatchObject({
          identity: { commandId },
          busy: false,
          unknown: true,
          error: expected.display,
        });
      }
      expect(ids, label).toEqual(earlierUnknown ? [commandId, commandId] : [commandId]);
      expect(displayed, label).toEqual(
        earlierUnknown ? ["request_timeout", expected.display] : [expected.display],
      );
    } finally {
      harness.dispose();
    }
  }
}

interface HistoryRetry {
  readonly target: "session" | "write";
  readonly status: number;
  readonly body: string | Uint8Array<ArrayBuffer>;
  readonly display: string;
  readonly invalidEnvelope?: true;
  readonly clears: boolean;
}

/** 让同一模板命令先失去回执，再通过真实 CSRF 复核和模板写入入口重试。 */
export async function runMailTemplateHistory(
  label: string,
  retries: readonly HistoryRetry[],
  firstUnknownReply?: Pick<HistoryRetry, "status" | "body" | "display">,
): Promise<void> {
  const restoreRequest = installBrowserRequest();
  try {
    const fetch = spyOnBrowserFetch();
    try {
      await withPendingCommands(SESSION, async (host, commands, checks) => {
        let reply: HistoryRetry | null =
          firstUnknownReply === undefined
            ? null
            : { ...firstUnknownReply, target: "write", clears: false };
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
        const failures: Array<AdminWriteFailure | null> = [];
        const entity = `mail-template:${TEMPLATE_KEY}`;
        let prepare: ReturnType<typeof prepareMailTemplateAttemptOnce> | null = null;
        expect(
          commands.submit(
            entity,
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
              return result;
            },
            () => {},
            () => {},
          ),
          label,
        ).toBe(true);
        await waitForAttempt(() => commands.isBusy(entity));
        expect(host.entries.peek(), label).toHaveLength(1);
        const initial = host.entries.peek()[0];
        if (initial === undefined) throw new Error("未登记首次命令");
        const { commandId } = initial.identity;
        expect(initial.unknown, label).toBe(true);
        expect(failures, label).toEqual([
          {
            kind: "outcome-unknown",
            code: firstUnknownReply?.display ?? "network_error",
            responseStatus: firstUnknownReply?.status ?? null,
          },
        ]);
        expect(initial.error, label).toBe(firstUnknownReply?.display ?? "network_error");
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
          await waitForAttempt(() => commands.isBusy(entity));
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
          const failure = failures[index + 1];
          if (failure == null) throw new Error(`未收到失败证据：${step}`);
          expect(failure.kind, step).toBe(
            preflight ? "not-sent" : retry.invalidEnvelope ? "outcome-unknown" : "verified-http",
          );
          expect(observedResponseStatus(failure), step).toBe(preflight ? null : retry.status);
          expect(adminWriteFailureDisplayCode(failure), step).toBe(retry.display);
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
