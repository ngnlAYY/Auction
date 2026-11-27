// 目录行操作栏的详情入口与代理快照可用性判定。

import { expect, onTestFinished, spyOn, test } from "bun:test";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { createCell } from "@auction/web-shared/reactive/cell";
import { err, ok } from "@auction/web-shared/result";
import { RouterContext } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import {
  type Context,
  children,
  createComponent,
  createEffect,
  createRoot,
  type Element,
  For,
  flush,
  Show,
  untrack,
} from "solid-js";

import type { OwnedProxy } from "../../api/account";
import type { CatalogRow } from "../../api/model";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "..";
import {
  createPublicBidRefresh,
  PublicBidRefreshContext,
  PublicRefreshGenerationContext,
} from "../board/refresh";
import { PublicCommandIdentityContext } from "../commands/pending";
import { activeOwnedProxy } from "../commands/proxy";
import { CommandDialogContext, type CommandDialogInput } from "./command-scope";
import {
  type CatalogDetailBid,
  CatalogDetailDialogContext,
  type CatalogDetailInput,
} from "./detail";
import { CatalogProxySnapshotsContext, createCatalogProxySnapshots } from "./proxy-snapshots";

const {
  CatalogProxyScope,
  detailBidReason,
  detailEntryAction,
  LiveCatalogActions,
  ordinaryBidBlocked,
  usableProxySnapshot,
} = await import("./actions");

const FIRST_USER = "00000000-0000-4000-8000-000000000001";
const SECOND_USER = "00000000-0000-4000-8000-000000000002";
const AUCTION_ID = "00000000-0000-4000-8000-000000000003";
const OTHER_AUCTION_ID = "00000000-0000-4000-8000-000000000004";
const LOT_ID = "00000000-0000-4000-8000-000000000005";
const CREATED_AT = "2026-11-13T00:00:00Z";
const EQUIPMENT = { name: "Legendary Shield", category: "shield" };

const WAITING_PROXY_WIRE = {
  authorization_id: "00000000-0000-4000-8000-000000000006",
  auction_id: AUCTION_ID,
  lot_id: LOT_ID,
  lot_number: 7,
  authorization_sequence: 1,
  usage_number: 2,
  mode: "slow",
  maximum_amount: 150_000,
  quota: { used: 2, limit: 3 },
  transitions: [
    {
      transition_number: 1,
      kind: "created",
      old_maximum_amount: null,
      new_maximum_amount: 150_000,
      old_mode: null,
      new_mode: "slow",
      old_status: null,
      new_status: "active",
      reason: null,
      resulting_state_version: 1,
      created_at: CREATED_AT,
    },
  ],
  status: "active",
  state_version: 1,
  created_at: CREATED_AT,
  updated_at: CREATED_AT,
  equipment: EQUIPMENT,
  bid: {
    id: "00000000-0000-4000-8000-000000000007",
    auction_id: AUCTION_ID,
    lot_id: LOT_ID,
    lot_number: 7,
    amount: 110_000,
    accepted_state_version: 1,
    fact_index: 1,
    mechanism: "proxy",
    created_at: CREATED_AT,
    leading: false,
    equipment: EQUIPMENT,
  },
  capabilities: { can_lower: true, waiting: true, at_maximum: false },
} satisfies Omit<OwnedProxy, "terminal_reason" | "invalid_reason">;

const WAITING_PROXY: OwnedProxy = {
  ...WAITING_PROXY_WIRE,
  terminal_reason: null,
  invalid_reason: null,
};

test("detail_entry_keeps_proxy_management_reachable_for_leading_bidders：领先者仍可进入代理页签管理授权", () => {
  // 普通出价可用：进直接出价，代理由弹窗页签切换。
  expect(detailEntryAction(false, true, false)).toBe("bid");
  // 回归：领先者的出价入口被禁用后，仍要能进入代理页签降低已有授权。
  expect(detailEntryAction(true, true, true)).toBe("proxy");
  // 达到金额上限的非领先者没有可执行的操作；拍品不可竞价时一律禁用。
  expect(detailEntryAction(true, true, false)).toBeNull();
  expect(detailEntryAction(true, false, true)).toBeNull();
  expect(detailEntryAction(true, false, false)).toBeNull();
});

test("等待反击的活动授权禁用普通出价，详情可进入代理管理", () => {
  const active = activeOwnedProxy(ok(WAITING_PROXY));
  const blocked = ordinaryBidBlocked(false, false, active);
  expect(blocked).toBe(true);
  expect(detailEntryAction(blocked, true, false, active !== null)).toBe("proxy");
  expect(detailEntryAction(blocked, false, false, active !== null)).toBeNull();
});

test("未读到生效授权时，普通出价仍按原有条件开放", () => {
  for (const snapshot of [
    null,
    err("network_error"),
    ok(null),
    ok({ ...WAITING_PROXY, status: "exhausted" }),
  ]) {
    const active = activeOwnedProxy(snapshot);
    const blocked = ordinaryBidBlocked(false, false, active);
    expect(blocked).toBe(false);
    expect(detailEntryAction(blocked, true, false, active !== null)).toBe("bid");
  }
});

test("详情在代理入口可用时仍显示普通出价受限的说明", () => {
  const input: CommandDialogInput = {
    lotId: WAITING_PROXY.lot_id,
    lotCode: "S07",
    lotName: WAITING_PROXY.equipment.name,
    action: "proxy",
    canBid: true,
    currentPrice: 120_000,
    nextBid: 126_000,
    lotStatus: "live",
    allowSlow: true,
    proxyDefaultMode: "slow",
    ordinaryBlocked: true,
    leading: false,
    blockedReason: "proxy-active-exists",
    proxy: ok(WAITING_PROXY),
    proxyReadGeneration: null,
  };
  expect(detailBidReason({ kind: "command", input, disabled: false })).toBe(input.blockedReason);
  expect(
    detailBidReason({ kind: "command", input: { ...input, proxy: ok(null) }, disabled: false }),
  ).toBe("");
  expect(detailBidReason({ kind: "login" })).toBe("");
});

test("已确认身份使用真实读取的成功或失败结果", () => {
  const success = ok(WAITING_PROXY);
  const failure = err("network_error");
  expect(usableProxySnapshot(success, true, true)).toBe(success);
  // 读取失败也要交给表单，由它展示错误与重试入口。
  expect(usableProxySnapshot(failure, true, true)).toBe(failure);
  expect(usableProxySnapshot(success, false, true)).toBeNull();
});

test("关闭或重读期间保留上一次成功读取", () => {
  const success = ok(WAITING_PROXY);
  expect(usableProxySnapshot(success, true, false)).toBe(success);
});

test("首次成功读取前区分没有快照与确认无授权", () => {
  expect(usableProxySnapshot(null, true, true)).toBeNull();
  expect(usableProxySnapshot(null, true, false)).toBeNull();
  const empty = ok(null);
  expect(usableProxySnapshot(empty, true, true)).toBe(empty);
});

test("重试时收起上一次读取错误", () => {
  expect(usableProxySnapshot(err("network_error"), true, false)).toBeNull();
});

function provide<T>(context: Context<T>, value: T) {
  return (render: () => Element) => () =>
    createComponent(context, {
      value,
      get children() {
        return render();
      },
    });
}

interface ProxyRequest {
  readonly userId: string | null;
  reply(authorization: typeof WAITING_PROXY_WIRE | null): void;
}

function interceptProxyReads(): ProxyRequest[] {
  const requests: ProxyRequest[] = [];
  const fetch = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      (input: RequestInfo | URL, init?: RequestInit) => {
        const path = input instanceof Request ? input.url : input.toString();
        if (path !== `/api/v1/lots/${LOT_ID}/proxy-authorizations/mine` || init?.signal == null) {
          throw new Error(`意外的代理请求 ${path}`);
        }
        const signal = init.signal;
        const { promise, resolve, reject } = Promise.withResolvers<Response>();
        const abort = () => reject(signal.reason);
        signal.addEventListener("abort", abort, { once: true });
        requests.push({
          userId: new Headers(init.headers).get(PUBLIC_EXPECTED_USER_ID_HEADER),
          reply(authorization) {
            signal.removeEventListener("abort", abort);
            resolve(Response.json({ code: "ok", message: "ok", data: { authorization } }));
          },
        });
        return promise;
      },
      { preconnect: () => {} },
    ),
  );
  onTestFinished(() => fetch.mockRestore());
  return requests;
}

function authenticated(userId: string): PublicSessionResult {
  return ok({
    authenticated: true,
    user_id: userId,
    username: "viewer",
    expires_at: null,
    absolute_expires_at: null,
  });
}

const LOT: CatalogRow = {
  lotId: LOT_ID,
  code: "S07",
  name: EQUIPMENT.name,
  gameEquipmentUrl: "",
  category: EQUIPMENT.category,
  equipmentGroup: "rare",
  status: "live",
  sellerName: "seller",
  level: null,
  primaryAttributeInitials: null,
  avgRoll: null,
  startPrice: 50_000,
  currentPrice: 120_000,
  nextBid: 126_000,
  leaderName: "another player",
  remaining: "",
};

function mountActions(row: { readonly canBid?: boolean; readonly nextBid?: number } = {}) {
  const requests = interceptProxyReads();
  return createRoot((dispose) => {
    const userId = createCell(FIRST_USER);
    const snapshots = createCatalogProxySnapshots();
    snapshots.replaceIdentity(FIRST_USER);
    const auction = createCell(AUCTION_ID);
    const snapshot = createCell(authenticated(FIRST_USER));
    const session: PublicSessionResource = Object.assign(snapshot, {
      state: (): "ready" => "ready",
      restart: () => {},
      cancel: () => {},
      clear: () => {},
    });
    const detail = createCell<CatalogDetailInput | null>(null);
    const detailBid = createCell<CatalogDetailBid>({ kind: "unavailable" });
    const page = createCell(1);
    const providers = [
      provide(RouterContext, {
        current: () => ({ kind: "not-found", segments: [] }),
        currentLocationString: () => "/",
        currentEntryKey: () => "test-entry",
        push: () => {},
        replace: () => {},
        goBack: () => {},
        restoreScrollAfterCommit: () => {},
      }),
      provide(PublicSessionResourceContext, session),
      provide(PublicCommandOwnerContext, userId),
      provide(PublicRefreshGenerationContext, createCell(0)),
      provide(PublicBidRefreshContext, createPublicBidRefresh()),
      provide(CommandDialogContext, createCell<CommandDialogInput | null>(null)),
      provide(CatalogDetailDialogContext, detail),
      provide(CatalogProxySnapshotsContext, snapshots),
    ];
    const render = () =>
      For({
        get each() {
          return [{ userId: userId(), auction: auction() }];
        },
        keyed: (current) => `${current.userId}:${current.auction}`,
        children: (current) =>
          createComponent(PublicCommandIdentityContext, {
            value: { userId: untrack(current).userId },
            get children() {
              return createComponent(CatalogProxyScope, {
                auctionId: untrack(current).auction,
                get children() {
                  return Show({
                    get when() {
                      return page() === 1;
                    },
                    get children() {
                      return untrack(() =>
                        LiveCatalogActions({
                          lotId: LOT_ID,
                          lotCode: LOT.code,
                          lotName: LOT.name,
                          canBid: row.canBid ?? true,
                          currentPrice: LOT.currentPrice,
                          nextBid: row.nextBid ?? LOT.nextBid,
                          lotStatus: LOT.status,
                          allowSlow: true,
                          proxyDefaultMode: "slow",
                          sessionReady: true,
                          authenticated: true,
                          leading: false,
                          detailBid: { cell: detailBid },
                        }),
                      );
                    },
                  });
                },
              });
            },
          }),
      });
    createEffect(children(providers.reduceRight((child, wrap) => wrap(child), render)), () => {});
    return {
      dispose,
      requests,
      page,
      auction,
      detailBid,
      open: () => detail.set({ row: LOT, bid: detailBid.peek(), watchable: true }),
      close: () => detail.set(null),
      changeUser(next: string) {
        snapshots.replaceIdentity(next);
        snapshot.set(authenticated(next));
        userId.set(next);
      },
      action() {
        const bid = detailBid.peek();
        if (bid.kind !== "command") {
          throw new Error("目录行的命令入口没有挂载");
        }
        return [bid.input.ordinaryBlocked, bid.input.blockedReason];
      },
    };
  });
}

async function settleProxyRead(): Promise<void> {
  for (let step = 0; step < 32; step += 1) {
    await Promise.resolve();
  }
  flush();
}

test("关闭详情与翻回本页时保留等待反击授权、禁用与说明，新结果无授权后恢复", async () => {
  const actions = mountActions();
  try {
    flush();
    expect(actions.requests).toHaveLength(0);
    expect(actions.action()).toEqual([false, ""]);
    actions.open();
    await settleProxyRead();
    expect(actions.requests.map((request) => request.userId)).toEqual([FIRST_USER]);
    actions.requests[0]?.reply(WAITING_PROXY_WIRE);
    await settleProxyRead();
    const blocked = [true, ""];
    expect(actions.action()).toEqual(blocked);

    actions.close();
    await settleProxyRead();
    expect(actions.action()).toEqual(blocked);
    actions.page.set(2);
    flush();
    expect(actions.detailBid.peek()).toEqual({ kind: "unavailable" });
    actions.page.set(1);
    flush();
    expect(actions.action()).toEqual(blocked);
    expect(actions.requests).toHaveLength(1);

    actions.open();
    await settleProxyRead();
    expect(actions.requests).toHaveLength(2);
    actions.requests[1]?.reply(null);
    await settleProxyRead();
    expect(actions.action()).toEqual([false, ""]);
  } finally {
    actions.dispose();
  }
});

test("没有生效授权时，其他原因的说明仍然显示", async () => {
  const limited = mountActions({ nextBid: 1_000_000_001 });
  try {
    flush();
    limited.open();
    await settleProxyRead();
    limited.requests[0]?.reply(null);
    await settleProxyRead();
    expect(limited.action()).toEqual([true, t("bid-limit-note")]);
  } finally {
    limited.dispose();
  }
});

test("换号销毁旧快照，场次之间隔离同一账号读取到的授权", async () => {
  const actions = mountActions();
  try {
    flush();
    actions.open();
    await settleProxyRead();
    actions.requests[0]?.reply(WAITING_PROXY_WIRE);
    await settleProxyRead();
    expect(actions.action()).toEqual([true, ""]);
    actions.close();
    actions.changeUser(SECOND_USER);
    flush();
    expect(actions.action()).toEqual([false, ""]);
    expect(actions.requests).toHaveLength(1);

    actions.open();
    await settleProxyRead();
    expect(actions.requests.map((request) => request.userId)).toEqual([FIRST_USER, SECOND_USER]);
    actions.requests[1]?.reply(WAITING_PROXY_WIRE);
    await settleProxyRead();
    expect(actions.action()).toEqual([true, ""]);
    actions.close();
    actions.auction.set(OTHER_AUCTION_ID);
    flush();
    expect(actions.action()).toEqual([false, ""]);
    expect(actions.requests).toHaveLength(2);
    actions.auction.set(AUCTION_ID);
    flush();
    expect(actions.action()).toEqual([true, ""]);
    expect(actions.requests).toHaveLength(2);
  } finally {
    actions.dispose();
  }
});
