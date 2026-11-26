// “我的”页查看抽屉的出价投影：可出价的进行中拍品给出与目录相同的命令入口，其余失败关闭。

import { expect, onTestFinished, spyOn, test } from "bun:test";
import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { RouterContext } from "@auction/web-shared/router";
import {
  type Context,
  children,
  createComponent,
  createEffect,
  createRoot,
  type Element,
  flush,
} from "solid-js";

import type { CatalogRow } from "../../../api/model";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../..";
import {
  createPublicBidRefresh,
  PublicBidRefreshContext,
  PublicRefreshGenerationContext,
} from "../../board/refresh";
import { PublicCommandIdentityContext } from "../../commands/pending";
import { CommandDialogContext, type CommandDialogInput } from "../command-scope";
import { CatalogProxySnapshotsContext, createCatalogProxySnapshots } from "../proxy-snapshots";
import { CatalogDetailDialogContext, type CatalogDetailInput } from ".";
import { LotViewRequestContext } from "./lot-view";

const { CatalogProxyScope } = await import("../actions");
const { LookupBidProjection, lookupNeedsAuction } = await import("./lookup-bid");

const USER = "00000000-0000-4000-8000-000000000001";
const AUCTION_ID = "00000000-0000-4000-8000-000000000003";
const LOT_ID = "00000000-0000-4000-8000-000000000005";

const ROW: CatalogRow = {
  lotId: LOT_ID,
  code: "S07",
  name: "Legendary Shield",
  gameEquipmentUrl: "",
  category: "shield",
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

const AUCTION: CurrentAuctionItemResponse = {
  id: AUCTION_ID,
  world: "main",
  number: 12,
  status: "live",
  opens_at: "2026-11-01T00:00:00Z",
  scheduled_ends_at: "2037-12-01T00:00:00Z",
  presentation_mode: "live",
  bidding_open: true,
  preview_revision: 0,
  proxy_default_mode: "fast",
};

function provide<T>(context: Context<T>, value: T) {
  return (render: () => Element) => () =>
    createComponent(context, {
      value,
      get children() {
        return render();
      },
    });
}

function mount(row: CatalogRow, auction: CurrentAuctionItemResponse | null) {
  // 抽屉打开后会读取本人代理；本测试只关心出价投影，读取保持挂起。
  const fetch = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(() => new Promise<Response>(() => {}), { preconnect: () => {} }),
  );
  onTestFinished(() => fetch.mockRestore());
  return createRoot((dispose) => {
    const snapshots = createCatalogProxySnapshots();
    snapshots.replaceIdentity(USER);
    const snapshot = createCell<PublicSessionResult>(
      ok({
        authenticated: true,
        user_id: USER,
        username: "viewer",
        expires_at: null,
        absolute_expires_at: null,
      }),
    );
    const session: PublicSessionResource = Object.assign(snapshot, {
      state: (): "ready" => "ready",
      restart: () => {},
      cancel: () => {},
      clear: () => {},
    });
    const detail = createCell<CatalogDetailInput | null>(null);
    const dialog = createCell<CommandDialogInput | null>(null);
    const providers = [
      provide(RouterContext, {
        current: () => ({ kind: "not-found", segments: [] }),
        currentLocationString: () => "/me",
        currentEntryKey: () => "test-entry",
        push: () => {},
        replace: () => {},
        goBack: () => {},
        restoreScrollAfterCommit: () => {},
      }),
      provide(PublicSessionResourceContext, session),
      provide(PublicCommandOwnerContext, createCell<string | null>(USER)),
      provide(PublicRefreshGenerationContext, createCell(0)),
      provide(PublicBidRefreshContext, createPublicBidRefresh()),
      provide(LotViewRequestContext, createCell(null)),
      provide(CommandDialogContext, dialog),
      provide(CatalogDetailDialogContext, detail),
      provide(CatalogProxySnapshotsContext, snapshots),
      provide(PublicCommandIdentityContext, { userId: USER }),
    ];
    const render = () =>
      createComponent(CatalogProxyScope, {
        auctionId: AUCTION_ID,
        get children() {
          return createComponent(LookupBidProjection, { row, auction, ready: true });
        },
      });
    createEffect(children(providers.reduceRight((child, wrap) => wrap(child), render)), () => {});
    flush();
    return { dispose, detail, dialog };
  });
}

test("lookup_drawer_offers_the_catalog_bid_entry_for_a_biddable_live_lot：可出价的进行中拍品给出目录同款命令入口", () => {
  const mounted = mount(ROW, AUCTION);
  try {
    const bid = mounted.detail.peek()?.bid;
    if (bid?.kind !== "command") throw new Error("抽屉没有出价命令入口");
    expect(bid.disabled).toBe(false);
    expect(bid.input).toMatchObject({
      lotId: LOT_ID,
      action: "bid",
      canBid: true,
      nextBid: 126_000,
      allowSlow: true,
      proxyDefaultMode: "fast",
    });
    // 点击入口写入页面级命令弹窗信号，由共享命令作用域承接提交。
    mounted.dialog.set(bid.input);
    expect(mounted.dialog.peek()?.lotId).toBe(LOT_ID);
  } finally {
    mounted.dispose();
  }
});

test("lookup_drawer_hides_the_bid_entry_when_the_lot_cannot_be_bid：不可出价时不提供入口", () => {
  for (const [row, auction] of [
    [ROW, { ...AUCTION, bidding_open: false }],
    [ROW, null],
    [{ ...ROW, status: "sold" }, AUCTION],
  ] as const) {
    const mounted = mount(row, auction);
    try {
      expect(mounted.detail.peek()?.bid).toEqual({ kind: "unavailable" });
    } finally {
      mounted.dispose();
    }
  }
});

test("lookup_drawer_reads_the_auction_for_an_ended_lot_only_with_an_open_command：已结束拍品仅在有未结命令时读取场次以支撑恢复入口", () => {
  expect(lookupNeedsAuction(false, true)).toBe(true);
  expect(lookupNeedsAuction(false, false)).toBe(false);
});
