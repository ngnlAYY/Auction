// 用 Rust 契约样本和内存传输验证各接口在实际入口应用整数词法。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";
import { adminAuditFilterDefault, fetchAdminAudit } from "./audit";
import { fetchDashboard } from "./dashboard";
import { adminApiFailureDefiniteRejection, adminApiFailureDisplayCode } from "./failure";
import { addAdminIntake, prepareAdminIntakeOnce } from "./intake";
import { fetchAdminLots, prepareAdminLotUpdateOnce, updateAdminLot } from "./lots";
import { fetchMailState } from "./mail";
import { fetchAdminProxyEvents } from "./proxy-events";
import { fetchRealtimeEventPage, fetchRealtimeFailurePage, fetchRealtimeLagPage } from "./realtime";
import { prepareRealtimeReplay, replayRealtimeFailure } from "./realtime/replay";
import { fetchAdminRenames, prepareAdminRenameRevert, revertAdminRename } from "./renames";
import {
  fetchSettlementReturn,
  fetchSettlementReturns,
  prepareSettlementReturnActionOnce,
  settlementReturnFiltersDefault,
  submitSettlementReturnAction,
} from "./returns";
import { fetchAdminRules, prepareAdminRulesPublish, publishAdminRules } from "./rules";
import {
  fetchRuntimeSwitches,
  prepareExternalAccessPauseOnce,
  submitExternalAccessPause,
} from "./runtime-switches";
import { type AdminSessionHost, registerAdminSessionHost } from "./session";
import { fetchAdminSettlement } from "./settlement";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const COMMAND_ID = "00000000-0000-4000-8000-000000000007";
const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const RETURN_ID = "00000000-0000-4000-8000-000000000024";
const INVALID_RESPONSE = "invalid_response";
const INVALID_WRITE_RECEIPT = { status: null, code: INVALID_RESPONSE };
const SESSION_PATH = "/api/v1/admin/session?include=server_time";
const SESSION_BODY = adminContract("session");

type Run<T> = (host: AdminSessionHost, signal: AbortSignal) => Promise<T>;
type RecordedRequest = { readonly method: string; readonly url: string; readonly body: string };

function adminContract(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

function requireOk<T>(result: Result<T, unknown>): T {
  if (!result.ok) throw new Error(`样本应通过，实际错误 ${JSON.stringify(result.error)}`);
  return result.value;
}

function object(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("样本字段应为 JSON 对象");
  }
  // 上方已确认非数组对象，调用方只传共享解析器生成的 JSON 值。
  return value as Record<string, unknown>;
}

function valueAt(value: unknown, path: string): unknown {
  let current = value;
  for (const key of path.split("/").slice(1)) {
    current = Array.isArray(current) ? current[Number(key)] : object(current)[key];
  }
  return current;
}

function setAt(value: unknown, path: string, next: unknown): void {
  const separator = path.lastIndexOf("/");
  const target = object(valueAt(value, path.slice(0, separator)));
  const field = path.slice(separator + 1);
  expect(Object.hasOwn(target, field)).toBe(true);
  target[field] = next;
}

async function withReplies<T>(
  bodies: readonly string[],
  run: (
    host: AdminSessionHost,
    signal: AbortSignal,
    requests: readonly RecordedRequest[],
  ) => Promise<T>,
): Promise<T> {
  const originalFetch = globalThis.fetch;
  const OriginalRequest = globalThis.Request;
  const session = requireOk(parseJson(SESSION_BODY));
  const actor = valueAt(session, "/data/actor");
  const csrfToken = valueAt(session, "/data/csrf_token");
  if (typeof actor !== "string" || typeof csrfToken !== "string") {
    throw new Error("会话样本缺少身份或 CSRF token");
  }
  const { host, release } = registerAdminSessionHost({
    check() {},
    writesAllowed: () => true,
    confirmed: () => ({
      actor,
      csrfToken,
      csrf_token: csrfToken,
      expiresAt: 1n,
      absoluteExpiresAt: 2n,
      serverTime: 0n,
      receivedAtMillis: 0,
    }),
  });
  const requests: RecordedRequest[] = [];
  try {
    globalThis.Request = class extends OriginalRequest {
      constructor(input: ConstructorParameters<typeof Request>[0], init?: RequestInit) {
        super(
          typeof input === "string" ? new URL(input, "https://auction.test").href : input,
          init,
        );
      }
    };
    globalThis.fetch = Object.assign(
      async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
        const url =
          input instanceof URL ? input.href : typeof input === "string" ? input : input.url;
        const responseBody = bodies[requests.length];
        requests.push({
          method: init?.method ?? "GET",
          url,
          body: typeof init?.body === "string" ? init.body : "",
        });
        if (responseBody === undefined) throw new Error(`未预期的请求 ${url}`);
        return new Response(responseBody);
      },
      {
        preconnect() {
          throw new Error("请求替身不应预连接");
        },
      },
    );
    return await run(host, new AbortController().signal, requests);
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.Request = OriginalRequest;
    release();
  }
}

function readAdminFixture<T>(body: string, path: string, read: Run<T>): Promise<T> {
  return withReplies([body], async (host, signal, requests) => {
    const result = await read(host, signal);
    expect(requests).toEqual([{ method: "GET", url: path, body: "" }]);
    return result;
  });
}

function decimalField(body: string, path: string): string {
  const value = requireOk(parseJson(body));
  const integer = valueAt(value, path);
  if (typeof integer !== "number" || !Number.isSafeInteger(integer)) {
    throw new Error(`样本字段 ${path} 必须是安全整数`);
  }
  // 把占位字符串换回原文数字，确保只改目标字段的词法。
  const placeholder = "__lexical_decimal__";
  expect(body).not.toContain(placeholder);
  setAt(value, path, placeholder);
  return JSON.stringify(value).replace(JSON.stringify(placeholder), `${integer}.0`);
}

async function verifyLexical<E>(
  body: string,
  field: string,
  invoke: (body: string) => Promise<Result<unknown, E>>,
  error: E,
) {
  expect((await invoke(body)).ok).toBe(true);
  expect(await invoke(decimalField(body, field))).toEqual(err(error));
}

function verifyRead<E>(
  body: string,
  field: string,
  path: string,
  read: Run<Result<unknown, E>>,
  error: E,
) {
  return verifyLexical(body, field, (text) => readAdminFixture(text, path, read), error);
}

function writeFixture<T>(body: string, method: string, path: string, write: Run<T>): Promise<T> {
  return withReplies([SESSION_BODY, body], async (host, signal, requests) => {
    const result = await write(host, signal);
    expect(requests.map(({ method, url }) => [method, url])).toEqual([
      ["GET", SESSION_PATH],
      [method, path],
    ]);
    return result;
  });
}

test("审计页拒绝分页总数的小数词法", async () => {
  await verifyRead(
    adminContract("audit"),
    "/data/total",
    "/api/v1/admin/audit?page=1&page_size=50",
    (host, signal) => fetchAdminAudit(host, 1, adminAuditFilterDefault(), 50, signal),
    INVALID_RESPONSE,
  );
});

test("录入准备按整数词法解析装备等级，允许浮点属性", () => {
  const body = '{"level":7,"stats":{"attack":{"value":"+7","base":7.0}}}';
  const prepare = (text: string) => prepareAdminIntakeOnce("json", "", text, 1000, "Player");
  expect(prepare(body)(COMMAND_ID).ok).toBe(true);
  expect(prepare(decimalField(body, "/level"))(COMMAND_ID)).toEqual(
    err({ status: null, code: "invalid_request" }),
  );
});

test("录入命令回执拒绝拍品编号的小数词法", async () => {
  await verifyLexical(
    adminContract("lot-intake-command"),
    "/data/lot_number",
    (text) =>
      writeFixture(text, "POST", "/api/v1/admin/worlds/main/auction-intake/items", (host, signal) =>
        addAdminIntake(
          host,
          COMMAND_ID,
          "main",
          1000,
          "Player",
          prepareAdminIntakeOnce(
            "url",
            "https://hentaiverse.org/equip/123/0123456789",
            "",
            1000,
            "Player",
          ),
          signal,
        ),
      ),
    INVALID_RESPONSE,
  );
});

test("代理历史拒绝数组行里版本的小数词法", async () => {
  await verifyRead(
    adminContract("proxy-events"),
    "/data/proxy_events/0/state_version",
    `/api/v1/admin/lots/${LOT_ID}/proxy-events?page=1&page_size=50`,
    (host, signal) => fetchAdminProxyEvents(host, LOT_ID, 1, 50, signal),
    INVALID_RESPONSE,
  );
});

test("拍品列表拒绝决定编辑版本的小数词法", async () => {
  await verifyRead(
    adminContract("lots"),
    "/data/lots/0/state_version",
    `/api/v1/admin/auctions/${AUCTION_ID}/lots?page=1&page_size=50`,
    (host, signal) => fetchAdminLots(host, AUCTION_ID, 1, 50, signal),
    INVALID_RESPONSE,
  );
});

test("拍品修改回执拒绝版本的小数词法", async () => {
  const lotId = "00000000-0000-4000-8000-000000000023";
  const prepare = prepareAdminLotUpdateOnce(1, "set_start_price", 2000);
  await verifyLexical(
    adminContract("lot-update-command"),
    "/data/lot/state_version",
    (text) =>
      writeFixture(text, "PATCH", `/api/v1/admin/lots/${lotId}`, (host, signal) =>
        updateAdminLot(host, lotId, COMMAND_ID, prepare, signal),
      ),
    INVALID_RESPONSE,
  );
});

test("邮件轮询状态拒绝暂停版本的小数词法", async () => {
  await verifyRead(
    adminContract("mail-diagnostics"),
    "/data/polling/state_version",
    "/api/v1/admin/mail?mailbox_key=main&include_diagnostics=true",
    (host, signal) => fetchMailState(host, "main", signal),
    INVALID_RESPONSE,
  );
});

const RENAME_LIST_PATH = "/api/v1/admin/renames?page=1&page_size=50";

function readRenames(host: AdminSessionHost, signal: AbortSignal) {
  return fetchAdminRenames(host, "all", "", 1, 50, signal);
}

test.each(["7.0", "7e0", "7"])("改名回滚按前置版本词法 %s 决定是否发送", async (version) => {
  const list = adminContract("renames");
  const page = requireOk(await readAdminFixture(list, RENAME_LIST_PATH, readRenames));
  const rename = page.renames[0];
  if (rename === undefined) throw new Error("样本缺少改名记录");
  const history = list
    .replace('"page_size": 50', '"page_size": 10')
    .replace('"user_state_version": 3', `"user_state_version": ${version}`);
  expect(history).toContain(`"user_state_version": ${version}`);
  const receipt = adminContract("rename-revert-command").replace(
    '"state_version": 4',
    '"state_version": 8',
  );
  await withReplies([history, SESSION_BODY, receipt], async (host, signal, requests) => {
    const prepared = await prepareAdminRenameRevert(host, COMMAND_ID, rename, signal);
    const historyRequest = {
      method: "GET",
      url: `/api/v1/admin/renames?page=1&page_size=10&user_id=${rename.user_id}`,
      body: "",
    };
    if (version === "7") {
      const result = requireOk(await revertAdminRename(host, requireOk(prepared), signal));
      expect(result.state_version).toBe(8);
      expect(requests).toEqual([
        historyRequest,
        { method: "GET", url: SESSION_PATH, body: "" },
        {
          method: "POST",
          url: `/api/v1/admin/renames/${rename.id}/revert`,
          body: `{"command_id":"${COMMAND_ID}","expected_state_version":7}`,
        },
      ]);
    } else {
      expect(prepared).toEqual(err("not_sent:invalid_response"));
      if (prepared.ok) throw new Error("异常版本进入了回滚准备结果");
      expect(adminApiFailureDefiniteRejection(prepared.error)).toBe(true);
      expect(adminApiFailureDisplayCode(prepared.error)).toBe("invalid_response");
      expect(requests).toEqual([historyRequest]);
    }
  });
});

test.each(["page", "page_size", "total"])("改名分页拒绝 %s 的小数词法", async (field) => {
  await verifyRead(
    adminContract("renames"),
    `/data/pagination/${field}`,
    RENAME_LIST_PATH,
    readRenames,
    INVALID_RESPONSE,
  );
});

test("空改名页接受分页总数 0 并拒绝 -0", async () => {
  const body = adminContract("renames-empty-page").replace('"total": 1', '"total": 0');
  const path = "/api/v1/admin/renames?page=2&page_size=1";
  const read = (host: AdminSessionHost, signal: AbortSignal) =>
    fetchAdminRenames(host, "all", "", 2, 1, signal);
  const page = requireOk(await readAdminFixture(body, path, read));
  expect(page.total).toBe(0);
  const negativeZero = body.replace('"total": 0', '"total": -0');
  expect(await readAdminFixture(negativeZero, path, read)).toEqual(err(INVALID_RESPONSE));
});

test("规则读取拒绝文档修订的小数词法", async () => {
  await verifyRead(
    adminContract("rules"),
    "/data/revision",
    "/api/v1/admin/rules?lang=en",
    (host, signal) => fetchAdminRules(host, "rules", "en", signal),
    INVALID_WRITE_RECEIPT,
  );
});

test("规则发布回执拒绝文档修订的小数词法", async () => {
  const body = adminContract("rules");
  const prepare = prepareAdminRulesPublish(
    5,
    "en",
    "Auction rules",
    "# Auction rules\n\nExample document.",
  );
  await verifyLexical(
    body,
    "/data/revision",
    (text) =>
      writeFixture(text, "PUT", "/api/v1/admin/rules", (host, signal) =>
        publishAdminRules(host, "rules", COMMAND_ID, prepare, signal),
      ),
    INVALID_WRITE_RECEIPT,
  );
});

test("结算报告拒绝单件拍品领先纳秒的小数词法", async () => {
  await verifyRead(
    adminContract("settlement-report"),
    "/data/lots/0/leader_held_nanos",
    `/api/v1/admin/auctions/${AUCTION_ID}/settlement-report`,
    (host, signal) => fetchAdminSettlement(host, AUCTION_ID, signal),
    INVALID_RESPONSE,
  );
});

test.each(["database_intent", "admin_applied"])(
  "运行时开关列表拒绝 %s 修订的小数词法",
  async (field) => {
    const body = requireOk(parseJson(adminContract("runtime-switches")));
    setAt(body, "/data/admin_last_success_age_seconds", 0.5);
    await verifyRead(
      JSON.stringify(body),
      `/data/${field}/0/revision`,
      "/api/v1/admin/runtime-switches",
      fetchRuntimeSwitches,
      INVALID_RESPONSE,
    );
  },
);

test("运行时开关写回执拒绝已接受修订的小数词法", async () => {
  const prepared = requireOk(
    prepareExternalAccessPauseOnce({
      commandId: COMMAND_ID,
      expectedRevision: 1,
      paused: false,
      expiresAt: null,
      reason: "resume regular polling",
    })(COMMAND_ID),
  );
  await verifyLexical(
    adminContract("runtime-switch-command"),
    "/data/accepted/revision",
    (text) =>
      writeFixture(
        text,
        "POST",
        "/api/v1/admin/runtime-switches/external-access-paused",
        (host, signal) => submitExternalAccessPause(host, prepared, signal),
      ),
    INVALID_WRITE_RECEIPT,
  );
});

test("退件列表拒绝案件版本的小数词法", async () => {
  await verifyRead(
    adminContract("settlement-returns"),
    "/data/returns/0/case_version",
    "/api/v1/admin/settlement-returns?page=1&page_size=50",
    (host, signal) =>
      fetchSettlementReturns(host, settlementReturnFiltersDefault(), 1n, 50, signal),
    INVALID_RESPONSE,
  );
});

test("退件详情拒绝摘要里案件版本的小数词法", async () => {
  await verifyRead(
    adminContract("settlement-return"),
    "/data/summary/case_version",
    `/api/v1/admin/settlement-returns/${RETURN_ID}`,
    (host, signal) => fetchSettlementReturn(host, RETURN_ID, signal),
    INVALID_RESPONSE,
  );
});

test("退件处理回执拒绝案件版本的小数词法", async () => {
  const submission = prepareSettlementReturnActionOnce({
    returnId: RETURN_ID,
    expectedVersion: 1,
    action: "acknowledge",
  });
  await verifyLexical(
    adminContract("settlement-return-command"),
    "/data/case_version",
    (text) =>
      writeFixture(
        text,
        "POST",
        `/api/v1/admin/settlement-returns/${RETURN_ID}/handle`,
        (host, signal) => submitSettlementReturnAction(host, COMMAND_ID, submission, signal),
      ),
    INVALID_WRITE_RECEIPT,
  );
});

test("看板拒绝邮件轮询版本的小数词法", async () => {
  const dashboard = requireOk(parseJson(adminContract("dashboard-collection-wait")));
  const mail = requireOk(parseJson(adminContract("mail")));
  const poller = valueAt(mail, "/data/polling");
  const observedAt = valueAt(mail, "/data/polling/updated_at");
  setAt(dashboard, "/data/mail", {
    status: "ok",
    observed_at: observedAt,
    data: { observed_at: observedAt, pollers: [poller] },
  });
  await verifyRead(
    JSON.stringify(dashboard),
    "/data/mail/data/pollers/0/state_version",
    "/api/v1/admin/dashboard?include=collection_wait",
    fetchDashboard,
    INVALID_RESPONSE,
  );
});

test("实时事件列表拒绝事件版本的小数词法", async () => {
  await verifyRead(
    adminContract("realtime-events"),
    "/data/events/0/schema_version",
    "/api/v1/admin/realtime/events?page=1&page_size=50",
    (host, signal) => fetchRealtimeEventPage(host, 1, "", "", 50, signal),
    INVALID_RESPONSE,
  );
});

test("实时滞后列表拒绝偏移的小数词法", async () => {
  await verifyRead(
    adminContract("realtime-lag"),
    "/data/lags/0/offset",
    "/api/v1/admin/realtime/lag?page=1&page_size=50",
    (host, signal) => fetchRealtimeLagPage(host, 1, "", "", 50, signal),
    INVALID_RESPONSE,
  );
});

test("实时失败列表拒绝分页总数的小数词法", async () => {
  await verifyRead(
    adminContract("realtime-failures"),
    "/data/pagination/total",
    "/api/v1/admin/realtime/failures?page=1&page_size=1",
    (host, signal) => fetchRealtimeFailurePage(host, 1, "", "", "", "", 1, signal),
    INVALID_RESPONSE,
  );
});

test("实时重放回执拒绝事件序号的小数词法", async () => {
  const events = requireOk(parseJson(adminContract("realtime-events")));
  const failures = requireOk(parseJson(adminContract("realtime-failures")));
  const failureId = valueAt(failures, "/data/failures/0/id");
  const originalEventId = valueAt(failures, "/data/failures/0/event_id");
  const event = object(valueAt(events, "/data/events/0"));
  if (typeof failureId !== "string" || typeof originalEventId !== "string") {
    throw new Error("失败列表样本缺少原事件或失败记录 ID");
  }
  const body = JSON.stringify({
    code: "ok",
    message: "",
    data: {
      replay: { ...event, event_id: COMMAND_ID, replay_of_event_id: originalEventId },
    },
  });
  const prepare = prepareRealtimeReplay(2, "{}");
  await verifyLexical(
    body,
    "/data/replay/stream_sequence",
    (text) =>
      writeFixture(
        text,
        "POST",
        `/api/v1/admin/realtime/failures/${failureId}/replay`,
        (host, signal) => replayRealtimeFailure(host, failureId, COMMAND_ID, prepare, signal),
      ),
    INVALID_WRITE_RECEIPT,
  );
});
