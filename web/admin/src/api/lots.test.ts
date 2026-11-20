// 对照 Rust 拍品目录与改价回执的契约测试。

import { expect, test } from "bun:test";
import {
  AdminLotListResponseSchema,
  AdminLotUpdateResponseSchema,
} from "@auction/contracts/dto/admin/lot";
import {
  AUCTION_ID,
  COMMAND_ID,
  checkedFixture,
  fixtureText,
  requestFixture,
  SESSION_REPLY,
  SESSION_REQUEST,
  success,
} from "./auction-contract-fixture.test";
import { fetchAdminLots, prepareAdminLotUpdateOnce, updateAdminLot } from "./lots";

test("lot_contract_fixtures", async () => {
  for (const [name, page, size, count] of [
    ["lots", 1, 50, 1],
    ["lots-empty-page", 2, 1, 0],
  ] as const) {
    const body = fixtureText(name);
    checkedFixture(body, AdminLotListResponseSchema, "/data/pagination/total");
    const { auction, pagination, lots } = success(
      await requestFixture(
        [{ body }],
        [["GET", `/api/v1/admin/auctions/${AUCTION_ID}/lots?page=${page}&page_size=${size}`]],
        (host, signal) => fetchAdminLots(host, AUCTION_ID, page, size, signal),
      ),
    );
    expect(auction.id).toBe(AUCTION_ID);
    expect(pagination).toEqual({ page, page_size: size, total: 1 });
    expect(lots).toHaveLength(count);
    if (count === 1) {
      expect(lots[0]).toMatchObject({
        auction_id: AUCTION_ID,
        external_mail_id: "9001",
        leader_user_id: null,
      });
    }
  }

  const body = fixtureText("lot-update-command");
  const {
    data: { lot },
  } = checkedFixture(body, AdminLotUpdateResponseSchema, "/data/lot/state_version");
  const result = success(
    await requestFixture(
      [SESSION_REPLY, { body }],
      [SESSION_REQUEST, ["PATCH", `/api/v1/admin/lots/${lot.id}`]],
      (host, signal) =>
        updateAdminLot(
          host,
          lot.id,
          COMMAND_ID,
          prepareAdminLotUpdateOnce(1, "set_start_price", 2000),
          signal,
        ),
    ),
  );
  expect([result.start_price, result.current_price, result.state_version]).toEqual([2000, 2000, 2]);
});
