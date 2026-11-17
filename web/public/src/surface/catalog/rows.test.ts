// 竞价历史把落败代理展开为紧跟胜者出价的代理行。

import { expect, test } from "bun:test";
import type { PublicBidLogEntryResponse } from "@auction/contracts/dto/public/catalog/history";

const { bidHistoryRows } = await import("./rows");

function proxyBid(
  id: string,
  bidderName: string,
  amount: number,
  valid: boolean,
): PublicBidLogEntryResponse {
  return {
    id,
    auction_id: "auction",
    lot_id: "lot",
    lot_number: 1,
    amount,
    equipment_name: "Legendary Staff",
    equipment_category: "Staff",
    bidder_name: bidderName,
    source: "proxy",
    source_post_number: null,
    source_post_url: null,
    valid,
    accepted_state_version: 1,
    fact_index: 1,
    mechanism: "proxy",
    created_at: "2026-11-14T12:57:20Z",
    defeated_proxy: null,
  };
}

test("defeated_proxy_follows_the_winning_bid_as_a_proxy_row：落败代理紧跟胜者出价展开为代理行", () => {
  const winner = proxyBid("bid-2", "winner", 15_750_000, true);
  winner.defeated_proxy = { bidder_name: "challenger", maximum_amount: 15_000_000 };
  const earlier = proxyBid("bid-1", "winner", 10_500_000, false);

  const rows = bidHistoryRows([winner, earlier]);

  const expectedWinner = proxyBid("bid-2", "winner", 15_750_000, true);
  expectedWinner.defeated_proxy = null;
  const expectedDefeated = proxyBid("bid-2-defeated", "challenger", 15_000_000, false);
  const expectedEarlier = proxyBid("bid-1", "winner", 10_500_000, false);
  expect(rows).toEqual([expectedWinner, expectedDefeated, expectedEarlier]);
});
