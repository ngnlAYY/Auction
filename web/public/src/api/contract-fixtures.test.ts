// 以 HTTP 生产序列化样本回归 Public 浏览器的严格解码。

import { expect, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { CurrentAuctionResponseSchema } from "@auction/contracts/dto/public/catalog/auction";
import { PublicCompensationMovementPageResponseSchema } from "@auction/contracts/dto/public/catalog/compensation";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createRoot, flush } from "solid-js";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
} from "../surface";
import { useWatchCommandOwner } from "../surface/watches/commands";
import {
  type CommandError,
  commandErrorOutcomeUncertain,
  createProxyAuthorization,
  lowerProxyAuthorization,
  placeBid,
} from "./commands";
import { decodeSuccess, object } from "./decoder";
import { decodeMyBids, decodeMyProxy } from "./decoder/account";
import { decodeBuildHello } from "./decoder/build-hello";
import {
  decodeAuctionList,
  decodeAuctionListV2,
  decodeBidLogPage,
  decodeBlacklist,
  decodeCompensationLots,
  decodeCompensationPool,
  decodeCurrentAuction,
  decodePriceHistoryPage,
  decodeRules,
  decodeWorlds,
} from "./decoder/catalog";
import { decodeFulfillmentPage } from "./decoder/fulfillment";
import { decodeAuctionRealtimePage, decodeRealtimeEvent } from "./decoder/realtime";
import { fetchForumSyncEnabled, fetchForumThread } from "./forum";
import { fetchRegistrationInfo } from "./registration";
import { fetchSiteDocument } from "./rules";
import { fetchPublicSession, loginPublic, logoutPublic } from "./session";
import { fetchSessionPreference, setSessionPreference } from "./session-preference";
import { fetchPublicReadiness } from "./status";
import { fetchLotWatchPreference, setLotWatchPreference } from "./watch-preference";
import { fetchLotWatches, setLotWatch } from "./watches";

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/public/",
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

function fixtureResponse(name: string, status = 200): TransportResponse {
  const text = fixtureText(name);
  return {
    status,
    statusText: "",
    headers: new Headers(),
    body: new TextEncoder().encode(text),
    text: () => text,
    json(policy) {
      const decoded = parseJson(text, policy);
      return decoded.ok ? decoded : err({ kind: "decode" });
    },
  };
}

test("当前场次仅在请求冻结代理模式时要求新字段", () => {
  const legacy = fixtureResponse("auction-current");
  const wire = decodeSuccess(
    legacy,
    CurrentAuctionResponseSchema,
    (value) => object(value) !== null,
  );
  expect(wire.ok && wire.value.auction?.proxy_default_mode).toBeNull();
  expect(decodeCurrentAuction(legacy, "main")).toEqual(err("invalid_response"));
  for (const [name, presentation] of [
    ["auction-current-proxy-mode", "live"],
    ["auction-current-opening", "opening"],
    ["auction-current-empty", undefined],
  ] as const) {
    const value = decodeCurrentAuction(fixtureResponse(name), "main");
    expect(value.ok && value.value.auction?.presentation_mode).toBe(presentation);
  }
});

test("v1 场次目录保持字段闭集，v2 接受可省略的论坛主题", () => {
  expect(decodeAuctionList(fixtureResponse("auctions-v1"), "main", "settled").ok).toBe(true);
  expect(decodeAuctionList(fixtureResponse("auctions-v1-live"), "main", "live").ok).toBe(true);
  const linked = fixtureResponse("auctions-v2");
  expect(decodeAuctionList(linked, "main", "settled")).toEqual(err("invalid_response"));
  const v2 = decodeAuctionListV2(linked, "main", "settled");
  expect(v2.ok && v2.value.auctions[0]?.forum_thread_url).toContain("showtopic=123");
  const live = decodeAuctionListV2(fixtureResponse("auctions-v2-live"), "main", "live");
  expect(live.ok && live.value.auctions[0]?.forum_thread_url).toBeNull();
});

test("竞价日志接受同刻事实与 Final 后的新版本", () => {
  const defaults = decodeBidLogPage(fixtureResponse("bid-log"));
  expect(defaults.ok && defaults.value.bids.every((bid) => bid.defeated_proxy === null)).toBe(true);
  const included = decodeBidLogPage(fixtureResponse("bid-log-defeated-proxy"));
  expect(included.ok && included.value.bids.map((bid) => bid.accepted_state_version)).toEqual([
    14, 13, 12,
  ]);
  if (!included.ok) throw new Error(included.error);
  const [latest, winner, previous] = included.value.bids;
  expect(latest?.created_at).not.toBe(winner?.created_at);
  expect(winner?.created_at).toBe(previous?.created_at);
  expect(latest?.valid).toBe(true);
  expect(winner?.valid).toBe(false);
  expect(previous?.valid).toBe(false);
  expect(winner?.fact_index).toBe(1);
  expect(winner?.defeated_proxy?.bidder_name).toBe("challenger");
});

test("成交历史接受有等级及无等级成交行", () => {
  const page = decodePriceHistoryPage(fixtureResponse("price-history"));
  expect(page.ok && page.value.rows[0]?.equipment_level).toBe(450);
  const unleveled = decodePriceHistoryPage(fixtureResponse("price-history-unleveled"));
  expect(unleveled.ok && unleveled.value.rows[0]?.equipment_level).toBeNull();
});

async function withFixture<T>(
  name: string,
  status: number,
  read: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const text = fixtureText(name);
  const network = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(async () => new Response(text, { status }), { preconnect: () => {} }),
  );
  try {
    return await read(new AbortController().signal);
  } finally {
    network.mockRestore();
  }
}

test("按请求身份解码本人竞价、Final 转换及缺省授权", () => {
  for (const [name, mode, version] of [
    ["my-bids", "slow", 1],
    ["my-bids-final", "fast", 2],
  ] as const) {
    const page = decodeMyBids(fixtureResponse(name));
    expect(page.ok && page.value.bids.length).toBe(1);
    expect(page.ok && page.value.proxies[0]?.mode).toBe(mode);
    expect(page.ok && page.value.proxies[0]?.state_version).toBe(version);
  }
  for (const [name, mode, lotId] of [
    ["my-proxy", "slow", LOT_ID],
    ["my-proxy-final", "fast", LOT_ID],
    ["my-proxy-empty", undefined, "00000000-0000-4000-8000-000000000016"],
  ] as const) {
    const proxy = decodeMyProxy(fixtureResponse(name), lotId);
    expect(proxy.ok && proxy.value.authorization?.mode).toBe(mode);
  }
});

test("履约回归包含付款截止、流拍与重寄", () => {
  for (const [name, statuses] of [
    ["fulfillment", ["awaiting_payment"]],
    ["fulfillment-recovery", ["unsold_return_pending", "awaiting_payment", "cod_resend_pending"]],
  ] as const) {
    const page = decodeFulfillmentPage(fixtureResponse(name), AUCTION_ID, "main", 1, 50);
    expect(page.ok && page.value.lots.map((row) => row.status)).toEqual([...statuses]);
    expect(decodeFulfillmentPage(fixtureResponse(name), AUCTION_ID, "isekai", 1, 50)).toEqual(
      err("invalid_response"),
    );
  }
});

test("补偿池与逐件明细接受冲正；流水使用契约 DTO", () => {
  for (const [name, count] of [
    ["compensation-pool", 0],
    ["compensation-pool-reversed", 1],
  ] as const) {
    const pool = decodeCompensationPool(fixtureResponse(name));
    expect(pool.ok && pool.value.rounds[0]?.reversal_count).toBe(count);
  }
  for (const [name, voided] of [
    ["compensation-lots", false],
    ["compensation-lots-reversed", true],
  ] as const) {
    const page = decodeCompensationLots(fixtureResponse(name));
    if (!page.ok) throw new Error(page.error);
    expect(page.value.lots[0]?.voided).toBe(voided);
  }
  const movements = decodeSuccess(
    fixtureResponse("compensation-movements"),
    PublicCompensationMovementPageResponseSchema,
    (value) => object(value) !== null,
  );
  expect(movements.ok && movements.value.movements.map((row) => row.kind)).toEqual([
    "opening",
    "tax",
    "compensation",
    "closing",
  ]);
});

test("回放页保留服务端游标与重读要求", () => {
  for (const [name, refetch] of [
    ["realtime-events", false],
    ["realtime-events-refetch", true],
  ] as const) {
    const page = decodeAuctionRealtimePage(fixtureResponse(name), AUCTION_ID);
    if (!page.ok) throw new Error(page.error);
    expect(page.value.next_after).toBe(2);
    expect(page.value.refetch_required).toBe(refetch);
    expect(page.value.events.length).toBe(refetch ? 0 : 1);
  }
});

test("编号页解码器拒绝合法但不属于本请求的游标投影", () => {
  expect(decodeAuctionList(fixtureResponse("auctions-v1-cursor"), "main", "settled")).toEqual(
    err("invalid_response"),
  );
  expect(decodeAuctionListV2(fixtureResponse("auctions-v2-cursor"), "main", "settled")).toEqual(
    err("invalid_response"),
  );
  expect(decodeBidLogPage(fixtureResponse("bid-log-cursor"))).toEqual(err("invalid_response"));
  expect(decodePriceHistoryPage(fixtureResponse("price-history-cursor"))).toEqual(
    err("invalid_response"),
  );
  expect(decodeCompensationPool(fixtureResponse("compensation-pool-cursor"))).toEqual(
    err("invalid_response"),
  );
});

test("WebSocket 首帧与 Final 事件均使用生产编码样本", () => {
  expect(decodeBuildHello(fixtureText("websocket-hello"))).toBe("fixture-build");
  const frame = decodeRealtimeEvent(fixtureText("websocket-event"), AUCTION_ID, "main");
  expect(frame.ok && frame.value.kind).toBe("auction.final_phase_entered");
  expect(frame.ok && frame.value.stream_sequence).toBe(2);
});

test("世界、黑名单与多语言站点文档接受服务端投影", async () => {
  const worlds = decodeWorlds(fixtureResponse("worlds"));
  if (!worlds.ok) throw new Error(worlds.error);
  expect(worlds.value.worlds.main?.enabled).toBe(true);
  expect(worlds.value.worlds.isekai?.enabled).toBe(false);
  for (const [name, count] of [
    ["blacklist", 1],
    ["blacklist-empty", 0],
  ] as const) {
    const page = decodeBlacklist(fixtureResponse(name));
    expect(page.ok && page.value.users.length).toBe(count);
  }
  for (const [name, title] of [
    ["rules", "Auction rules"],
    ["about", "About the auction"],
  ] as const) {
    const document = decodeRules(fixtureResponse(name));
    expect(document.ok && document.value.title).toBe(title);
    expect(document.ok && document.value.locale).toBe("en");
  }
  expect(
    await withFixture("about-unpublished", 404, (signal) =>
      fetchSiteDocument("about", "ja", signal),
    ),
  ).toEqual(err("about_unpublished"));
});

test("会话解码区分有限、永久和匿名", async () => {
  for (const [name, authenticated, timed] of [
    ["session", true, true],
    ["session-permanent", true, false],
    ["session-anonymous", false, false],
  ] as const) {
    const { result } = await withFixture(name, 200, (signal) =>
      fetchPublicSession(signal, false, () => true),
    );
    if (!result.ok) throw new Error(result.error);
    if (authenticated) {
      expect(result.value?.authenticated).toBe(true);
      expect(result.value?.expires_at !== null).toBe(timed);
    } else {
      expect(result.value).toBeNull();
    }
  }
});

test("账号偏好与标记集合使用权威会话身份", async () => {
  const user = AUCTION_ID;
  for (const [name, duration] of [
    ["session-preference", null],
    ["session-preference-timed", 3600],
  ] as const) {
    const result = await withFixture(name, 200, (signal) => fetchSessionPreference(user, signal));
    expect(result.ok && result.value.duration_seconds).toBe(duration);
  }
  for (const [name, channel, pm] of [
    ["lot-watch-preference", "mm", false],
    ["lot-watch-preference-both", "both", true],
  ] as const) {
    const result = await withFixture(name, 200, (signal) => fetchLotWatchPreference(user, signal));
    if (!result.ok) throw new Error(result.error.code);
    expect(result.value.channel).toBe(channel);
    expect(result.value.pm_available).toBe(pm);
  }
  for (const [name, watched] of [
    ["lot-watches", true],
    ["lot-watches-removed", false],
  ] as const) {
    const result = await withFixture(name, 200, (signal) =>
      fetchLotWatches(user, AUCTION_ID, signal),
    );
    if (!result.ok) throw new Error(result.error.code);
    expect(result.value.watches[0]?.watched).toBe(watched);
  }
});

test("论坛镜像与就绪探针区分可用、关闭和未就绪", async () => {
  for (const [name, count] of [
    ["registration-info", 1],
    ["registration-info-empty", 0],
  ] as const) {
    const info = await withFixture(name, 200, fetchRegistrationInfo);
    expect(info.ok && info.value.bot_players.length).toBe(count);
  }
  for (const [name, status] of [
    ["forum", "published"],
    ["forum-pending", "pending"],
  ] as const) {
    const thread = await withFixture(name, 200, (signal) => fetchForumThread(AUCTION_ID, signal));
    expect(thread.ok && thread.value.status).toBe(status);
  }
  expect(await withFixture("forum-sync", 200, fetchForumSyncEnabled)).toEqual({
    ok: true,
    value: true,
  });
  expect(await withFixture("forum-sync-disabled", 200, fetchForumSyncEnabled)).toEqual({
    ok: true,
    value: false,
  });
  expect(await withFixture("readiness", 200, fetchPublicReadiness)).toEqual({
    ok: true,
    value: true,
  });
  expect(await withFixture("readiness-unavailable", 503, fetchPublicReadiness)).toEqual({
    ok: true,
    value: false,
  });
});

async function withCookieFixture<T>(
  name: string,
  status: number,
  read: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const globals = {
    window: { isSecureContext: true },
    navigator: {
      locks: {
        request: async (
          lockName: string,
          _options: unknown,
          hold: (lock: { name: string }) => Promise<void>,
        ) => hold({ name: lockName }),
      },
    },
  };
  const descriptors = Object.keys(globals).map(
    (key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const,
  );
  // Bun 测试替身只授予锁，真正的请求和错误解码仍走生产入口。
  try {
    for (const [key, value] of Object.entries(globals)) {
      Object.defineProperty(globalThis, key, { configurable: true, value });
    }
    return await withFixture(name, status, read);
  } finally {
    for (const [key, descriptor] of descriptors) {
      if (descriptor === undefined) Reflect.deleteProperty(globalThis, key);
      else Object.defineProperty(globalThis, key, descriptor);
    }
  }
}

const COMMAND_ID = "00000000-0000-4000-8000-000000000030";
const AUTHORIZATION_ID = "00000000-0000-4000-8000-000000000012";

test("出价与代理使用后端错误信封区分明确拒绝和暂不可用", async () => {
  const bid = (signal: AbortSignal) => placeBid(AUCTION_ID, LOT_ID, 2_000, COMMAND_ID, signal);
  const create = (signal: AbortSignal) =>
    createProxyAuthorization(AUCTION_ID, LOT_ID, 2_000, "slow", COMMAND_ID, signal);
  const lower = (signal: AbortSignal) =>
    lowerProxyAuthorization(AUCTION_ID, LOT_ID, AUTHORIZATION_ID, 2_000, COMMAND_ID, signal);
  for (const [code, status, send] of [
    ["invalid_amount", 400, bid],
    ["bid_too_low", 409, bid],
    ["already_leading", 409, bid],
    ["active_proxy_exists", 409, bid],
    ["auction_not_open", 409, bid],
    ["bidder_ineligible", 403, bid],
    ["invalid_policy", 409, bid],
    ["command_conflict", 409, bid],
    ["temporarily_unavailable", 503, bid],
    ["session_changed", 409, bid],
    ["unauthenticated", 401, bid],
    ["invalid_maximum_amount", 400, create],
    ["invalid_proxy_mode", 400, create],
    ["proxy_disabled", 409, create],
    ["proxy_slow_final", 409, create],
    ["proxy_outbid_limit", 409, create],
    ["proxy_reentry_maximum_not_higher", 409, create],
    ["proxy_maximum_not_lower", 409, lower],
    ["proxy_maximum_below_binding", 409, lower],
    ["proxy_authorization_not_active", 409, lower],
  ] as const) {
    const result = await withFixture<Result<unknown, CommandError>>(
      `error-${code.replaceAll("_", "-")}`,
      status,
      (signal) => send(signal),
    );
    expect(result).toEqual(err({ code, status }));
    if (result.ok) throw new Error("预期命令被拒绝");
    expect(commandErrorOutcomeUncertain(result.error)).toBe(status === 503);
  }
});

test("标记与两项偏好从生产错误信封读取码和版本冲突", async () => {
  const watch = (signal: AbortSignal) =>
    setLotWatch(
      AUCTION_ID,
      AUCTION_ID,
      LOT_ID,
      { command_id: COMMAND_ID, watched: true, channel: "mm", expected_version: 1 },
      signal,
    );
  const watchPreference = (signal: AbortSignal) =>
    setLotWatchPreference(
      AUCTION_ID,
      { command_id: COMMAND_ID, channel: "both", expected_version: 1 },
      signal,
    );
  const sessionPreference = (signal: AbortSignal) =>
    setSessionPreference(
      AUCTION_ID,
      { command_id: COMMAND_ID, duration_seconds: 3600, expected_version: 1 },
      signal,
    );

  for (const [code, status, send] of [
    ["invalid_watch", 422, watch],
    ["watch_conflict", 409, watch],
    ["not_found", 404, watch],
    ["invalid_watch_preference", 422, watchPreference],
    ["watch_preference_conflict", 409, watchPreference],
    ["pm_channel_unavailable", 422, watchPreference],
    ["invalid_session_preference", 422, sessionPreference],
    ["session_preference_conflict", 409, sessionPreference],
  ] as const) {
    const result = await withCookieFixture<Result<unknown, CommandError>>(
      `error-${code.replaceAll("_", "-")}`,
      status,
      (signal) => send(signal),
    );
    expect(result).toEqual(err({ code, status }));
  }
});

test("标记重试保留冻结命令，直到收到结构合法的冲突信封", async () => {
  const read = parseJson(fixtureText("error-watch-conflict"));
  const envelope = read.ok ? object(read.value) : null;
  if (envelope === null) throw new Error("缺少标记冲突样本");
  const session: PublicSessionResource = Object.assign(
    createCell(
      ok({
        authenticated: true,
        user_id: AUCTION_ID,
        username: "ExamplePlayer",
        expires_at: null,
        absolute_expires_at: null,
      }),
    ),
    { state: () => "ready" as const, restart: () => {}, cancel: () => {}, clear: () => {} },
  );
  const previousOwner = PublicCommandOwnerContext.defaultValue;
  const previousSession = PublicSessionResourceContext.defaultValue;
  let mounted: { owner: ReturnType<typeof useWatchCommandOwner>; dispose: () => void };
  try {
    PublicCommandOwnerContext.defaultValue = () => AUCTION_ID;
    PublicSessionResourceContext.defaultValue = session;
    mounted = createRoot((dispose) => ({ owner: useWatchCommandOwner(AUCTION_ID), dispose }));
  } finally {
    PublicCommandOwnerContext.defaultValue = previousOwner;
    PublicSessionResourceContext.defaultValue = previousSession;
  }

  const requests: string[] = [];
  const responses = [true, [], null];
  const network = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (url: URL | RequestInfo, init?: RequestInit) => {
        expect(String(url)).toBe(`/api/v1/lots/${LOT_ID}/watch`);
        expect(init?.method).toBe("PUT");
        expect(new Headers(init?.headers).get(PUBLIC_EXPECTED_USER_ID_HEADER)).toBe(AUCTION_ID);
        const body = init?.body;
        if (typeof body !== "string" || requests.length >= responses.length) {
          throw new Error("非预期的标记提交");
        }
        const data = responses[requests.length];
        requests.push(body);
        return Response.json({ ...envelope, data }, { status: 409 });
      },
      { preconnect: () => {} },
    ),
  );
  const original = {
    command_id: COMMAND_ID,
    watched: true,
    channel: "mm",
    expected_version: 7,
  };
  const replacement = { ...original, watched: false, expected_version: 8 };
  try {
    for (const [attempt, payload] of [original, replacement, replacement].entries()) {
      mounted.owner.submit(AUCTION_ID, LOT_ID, "Sta12", payload);
      // 测试运行器可能在任一 await 之间交还执行权，等待宿主完成本次提交。
      for (let turn = 0; turn < 20 && mounted.owner.commands.peek().get(LOT_ID)?.busy; turn += 1) {
        await Bun.sleep(1);
        flush();
      }
      const pending = mounted.owner.commands.peek().get(LOT_ID);
      expect(pending?.payload).toEqual(original);
      expect(pending?.busy).toBe(false);
      expect(pending?.uncertain).toBe(attempt < 2);
      expect(pending?.error).toEqual({
        code: attempt < 2 ? "invalid_response" : "watch_conflict",
        status: 409,
      });
    }
    expect(requests).toEqual(Array.from({ length: 3 }, () => JSON.stringify(original)));
  } finally {
    mounted.dispose();
    network.mockRestore();
  }
});

test("会话错误保留后端明确拒绝码与公开封禁原因", async () => {
  for (const [code, status, reason] of [
    ["invalid_key", 400, null],
    ["invalid_credentials", 401, null],
    ["account_blacklisted", 403, "示例封禁原因"],
  ] as const) {
    const result = await withCookieFixture(`error-${code.replaceAll("_", "-")}`, status, (signal) =>
      loginPublic("fixture-placeholder", signal),
    );
    if (result.ok) throw new Error("预期登录被拒绝");
    expect(result.error.code).toBe(code);
    expect(result.error.publicReason).toBe(reason);
  }
  expect(
    await withCookieFixture("error-unauthenticated", 401, (signal) =>
      logoutPublic(AUCTION_ID, signal),
    ),
  ).toEqual(err("unauthenticated"));
});
