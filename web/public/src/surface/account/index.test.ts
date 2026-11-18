// 账户出价和代理刷新时，查看入口随记录标识复用。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createRoot, flush, mapArray } from "solid-js";

import { ownedBidRowKey, ownedProxyRowKey } from ".";

const FIRST_ROW = { id: "bid-1", authorization_id: "proxy-1", lot_id: "lot-1" };

test.each([
  ["出价", ownedBidRowKey],
  ["代理", ownedProxyRowKey],
] as const)("%s 列表按记录身份保留查看目标，同拍品的另一条记录另建行", (_name, keyed) => {
  const newer = { id: "bid-2", authorization_id: "proxy-2", lot_id: "lot-2" };
  const replacement = { ...FIRST_ROW, id: "bid-3", authorization_id: "proxy-3" };
  const rows = createCell([FIRST_ROW]);
  createRoot((dispose) => {
    try {
      const view = mapArray(rows, (row) => ({ viewLotId: () => row().lot_id }), { keyed });
      const [focused] = view();
      expect(focused?.viewLotId()).toBe("lot-1");

      rows.set([newer, { ...FIRST_ROW }]);
      flush();
      const [newRow, oldRow] = view();
      expect(oldRow).toBe(focused);
      expect(focused?.viewLotId()).toBe("lot-1");
      expect(newRow?.viewLotId()).toBe("lot-2");

      rows.set([newer, replacement]);
      flush();
      expect(view()[0]).toBe(newRow);
      expect(view()[1]).not.toBe(focused);
      expect(view()[1]?.viewLotId()).toBe("lot-1");
    } finally {
      dispose();
    }
  });
});
