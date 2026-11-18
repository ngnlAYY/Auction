// 完整竞价历史更新时，论坛出价和展开的落败代理各自保留展示身份。

import { expect, test } from "bun:test";
import type { PublicBidLogEntryResponse } from "@auction/contracts/dto/public/catalog/history";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createRoot, flush, mapArray } from "solid-js";

import { bidHistoryRows } from "../rows";
import { bidHistoryRowKey } from "./bid-history";

const FORUM_BID: PublicBidLogEntryResponse = {
  id: "00000000-0000-4000-8000-000000000012",
  auction_id: "00000000-0000-4000-8000-000000000001",
  lot_id: "00000000-0000-4000-8000-000000000002",
  lot_number: 1,
  amount: 1000,
  equipment_name: "Phase Robe",
  equipment_category: "cloth",
  bidder_name: "First",
  source: "forum",
  source_post_number: 3,
  source_post_url: "https://forums.e-hentai.org/index.php?showtopic=1&p=3#entry3",
  valid: true,
  accepted_state_version: 2,
  fact_index: 1,
  mechanism: "ordinary",
  created_at: "2026-01-02T00:00:00Z",
  defeated_proxy: null,
};

const PROXY_BID: PublicBidLogEntryResponse = {
  ...FORUM_BID,
  id: "00000000-0000-4000-8000-000000000011",
  bidder_name: "Proxy winner",
  amount: 900,
  source: "proxy",
  source_post_number: null,
  source_post_url: null,
  valid: false,
  accepted_state_version: 1,
  mechanism: "proxy",
  created_at: "2026-01-01T00:00:00Z",
  defeated_proxy: { bidder_name: "Defeated", maximum_amount: 800 },
};

test("插入新出价后复用原论坛记录与落败代理行，删除时释放对应行", () => {
  const newer = {
    ...FORUM_BID,
    id: "00000000-0000-4000-8000-000000000013",
    amount: 1100,
    bidder_name: "Second",
    source_post_number: 4,
    source_post_url: "https://forums.e-hentai.org/index.php?showtopic=1&p=4#entry4",
    accepted_state_version: 3,
    created_at: "2026-01-03T00:00:00Z",
  };
  const bids = createCell([FORUM_BID, PROXY_BID]);
  createRoot((dispose) => {
    try {
      const mapped = mapArray(
        () => bidHistoryRows(bids()),
        (row) => ({ record: row }),
        { keyed: bidHistoryRowKey },
      );
      const [focused, proxy, defeated] = mapped();
      expect(focused?.record().source_post_url).toBe(FORUM_BID.source_post_url);
      expect(defeated?.record().id).toBe(`${PROXY_BID.id}-defeated`);

      bids.set([newer, { ...FORUM_BID, valid: false }, { ...PROXY_BID }]);
      flush();
      const [newRow, oldRow, oldProxy, oldDefeated] = mapped();
      expect(oldRow).toBe(focused);
      expect(oldProxy).toBe(proxy);
      expect(oldDefeated).toBe(defeated);
      expect(oldRow?.record().source_post_url).toBe(FORUM_BID.source_post_url);
      expect(oldRow?.record().valid).toBe(false);
      expect(newRow?.record().source_post_url).toBe(newer.source_post_url);

      bids.set([newer]);
      flush();
      expect(mapped()).toHaveLength(1);
      expect(mapped()[0]).toBe(newRow);
    } finally {
      dispose();
    }
  });
});
