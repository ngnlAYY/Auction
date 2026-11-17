// Public 顶栏场次、倒计时与停用实时目标的展示判定。

import { expect, test } from "bun:test";
import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicConnectionState, PublicRealtimeTarget } from "./realtime";

const { publicConnectionState, tickerAuction, tickerCountdown } = await import("./realtime");

function auction(id: string, number: number, status: string): CurrentAuctionItemResponse {
  return {
    id,
    world: "main",
    number,
    status,
    opens_at: "2026-11-01T00:00:00Z",
    scheduled_ends_at: "2026-11-08T00:00:00Z",
    presentation_mode: "live",
    bidding_open: status === "live",
    preview_revision: 1,
    proxy_default_mode: "fast",
  };
}

test("ticker_labels_the_auction_that_the_page_displays：顶栏只标注页面正在展示的场次", () => {
  const current = auction("current", 12, "live");
  const historical = auction("historical", 8, "settled");
  const requested = { kind: "auction", id: "historical" } as const;

  // 没有显式场次请求的页面显示所选世界的当前场次。
  expect(tickerAuction(null, historical, current)).toBe(current);

  // 历史场次页显示正文同一场次，而不是当前场次的编号、状态与倒计时。
  expect(tickerAuction(requested, historical, current)).toBe(historical);
  // 尚未加载、加载失败或残留的是另一场次时不借用当前场次。
  expect(tickerAuction(requested, null, current)).toBeNull();
  const stale = auction("another", 7, "settled");
  expect(tickerAuction(requested, stale, current)).toBeNull();
});

test("ticker_counts_down_to_close_while_bidding_and_to_open_during_preview：竞价倒计到截止，预览倒计到开拍", () => {
  for (const status of ["live", "final"]) {
    expect(tickerCountdown(auction("a", 5, status))).toEqual({
      kind: "closing",
      endsAt: "2026-11-08T00:00:00Z",
    });
  }
  const preview = auction("a", 5, "preview");
  preview.scheduled_ends_at = null;
  expect(tickerCountdown(preview)).toEqual({
    kind: "opening",
    opensAt: "2026-11-01T00:00:00Z",
  });
  const liveWithoutEnd = auction("a", 5, "live");
  liveWithoutEnd.scheduled_ends_at = null;
  expect(tickerCountdown(liveWithoutEnd)).toBeNull();
});

test("ticker_hides_the_countdown_once_bidding_is_over：竞价结束后不再显示倒计时", () => {
  // 历史场次携带过去的截止时刻；不能再渲染成“落锤中…”。
  for (const status of ["settling", "settled", "archived", "cancelled", "draft"]) {
    expect(tickerCountdown(auction("a", 4, status))).toBeNull();
  }
});

test("inactive_targets_explain_why_no_realtime_connection_exists：停用目标说明为何没有实时连接", () => {
  const state = (authenticated: boolean, preview: boolean): PublicConnectionState => {
    const target: PublicRealtimeTarget = { kind: "inactive", world: "main", preview };
    return publicConnectionState("ready", false, authenticated, target, "manual");
  };
  expect(state(true, true)).toBe("preview");
  // 匿名用户开拍后也不会建立连接，与竞价期一致显示手动刷新。
  expect(state(false, true)).toBe("manual");
  expect(state(true, false)).toBe("idle");
  expect(state(false, false)).toBe("idle");
});
