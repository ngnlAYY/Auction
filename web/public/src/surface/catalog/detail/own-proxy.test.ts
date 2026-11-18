// 抽屉只在本人授权读取成功后展示代理摘要。

import { expect, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";

import type { OwnedProxy } from "../../../api/account";
import type { CatalogDetailBid } from ".";

const { ownProxy } = await import("./own-proxy");

function proxy(): OwnedProxy {
  return {
    authorization_id: "018f4c8e-7b2a-7c3d-8e4f-000000000001",
    auction_id: "018f4c8e-7b2a-7c3d-8e4f-000000000002",
    lot_id: "018f4c8e-7b2a-7c3d-8e4f-000000000003",
    lot_number: 7,
    authorization_sequence: 1,
    usage_number: 2,
    mode: "fast",
    maximum_amount: 250_000,
    quota: { used: 2, limit: 3 },
    transitions: [],
    status: "active",
    terminal_reason: null,
    state_version: 4,
    created_at: "2026-11-13T00:00:00Z",
    updated_at: "2026-11-13T00:00:00Z",
    equipment: { name: "Legendary Shield", category: "shield" },
    bid: null,
    invalid_reason: null,
    capabilities: { can_lower: true, waiting: false, at_maximum: false },
  };
}

function command(proxyResult: Result<OwnedProxy | null, string> | null): CatalogDetailBid {
  return {
    kind: "command",
    input: {
      lotId: "018f4c8e-7b2a-7c3d-8e4f-000000000003",
      lotCode: "S07",
      lotName: "Legendary Shield",
      action: "bid",
      canBid: true,
      currentPrice: 100_000,
      nextBid: 105_000,
      lotStatus: "live",
      allowSlow: true,
      proxyDefaultMode: "fast",
      ordinaryBlocked: false,
      leading: false,
      blockedReason: "",
      proxy: proxyResult,
      proxyReadGeneration: null,
    },
    disabled: false,
  };
}

test("drawer_shows_the_own_proxy_only_after_it_was_read：本人代理只在读取成功后展示", () => {
  // 回归：本人代理的上限与已用次数要在详情抽屉里可见。
  const loaded = command(ok(proxy()));
  const shown = ownProxy(loaded);
  if (shown === null) {
    throw new Error("loaded authorization is shown");
  }
  expect([shown.maximum_amount, shown.quota.used, shown.quota.limit]).toEqual([250_000, 2, 3]);

  // 读取中、读取失败与没有设置授权都不占位。
  expect(ownProxy(command(null))).toBeNull();
  expect(ownProxy(command(err("unauthenticated")))).toBeNull();
  expect(ownProxy(command(ok(null)))).toBeNull();
  // 匿名访客与没有操作栏的行没有授权快照。
  expect(ownProxy({ kind: "login" })).toBeNull();
  expect(ownProxy({ kind: "unavailable" })).toBeNull();
});
