// 对照 Rust 场次列表、创建与排期回执的契约测试。

import { expect, test } from "bun:test";
import {
  AdminAuctionLifecycleResponseSchema,
  AdminUpdateAuctionScheduleResponseSchema,
} from "@auction/contracts/dto/admin/auction";
import { AdminAuctionListResponseSchema } from "@auction/contracts/dto/admin/settlement";
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
import {
  createAdminAuction,
  fetchAdminAuctions,
  prepareAdminAuctionCreateOnce,
  prepareAdminAuctionScheduleOnce,
  updateAdminAuctionSchedule,
} from "./auctions";

test("auction_contract_fixtures", async () => {
  for (const [name, page, size, count] of [
    ["auctions", 1, 50, 1],
    ["auctions-empty-page", 2, 1, 0],
  ] as const) {
    const body = fixtureText(name);
    checkedFixture(body, AdminAuctionListResponseSchema, "/data/pagination/total");
    const { pagination, auctions } = success(
      await requestFixture(
        [{ body }],
        [["GET", `/api/v1/admin/auctions?page=${page}&page_size=${size}`]],
        (host, signal) => fetchAdminAuctions(host, "", page, size, signal),
      ),
    );
    expect(pagination).toEqual({ page, page_size: size, total: 1 });
    expect(auctions).toHaveLength(count);
    if (count === 1) {
      expect(auctions[0]).toMatchObject({ status: "draft", opened_at: null });
    }
  }

  const body = fixtureText("auction-create-command");
  const payload = checkedFixture(
    body,
    AdminAuctionLifecycleResponseSchema,
    "/data/auction/state_version",
  );
  expect(payload.code).toBe("created");
  const { auction } = payload.data;
  const result = success(
    await requestFixture(
      [SESSION_REPLY, { body, status: 201 }],
      [SESSION_REQUEST, ["POST", "/api/v1/admin/auctions"]],
      (host, signal) =>
        createAdminAuction(
          host,
          COMMAND_ID,
          auction.id,
          prepareAdminAuctionCreateOnce(
            auction.id,
            auction.world_id,
            auction.sequence,
            auction.preview_scheduled_at,
            auction.scheduled_open_at,
            "2025-11-15T13:46:40.110000Z",
          ),
          signal,
        ),
    ),
  );
  expect([result.status, result.state_version]).toEqual(["draft", 1]);
});

test("schedule_command_contract_fixture", async () => {
  const body = fixtureText("auction-schedule-command");
  const {
    data: { auction },
  } = checkedFixture(body, AdminUpdateAuctionScheduleResponseSchema, "/data/auction/state_version");
  const end = auction.scheduled_ends_at;
  if (end == null) throw new Error("排期样本缺少结束时间");
  const result = success(
    await requestFixture(
      [SESSION_REPLY, { body }],
      [SESSION_REQUEST, ["PATCH", `/api/v1/admin/auctions/${AUCTION_ID}/schedule`]],
      (host, signal) =>
        updateAdminAuctionSchedule(
          host,
          COMMAND_ID,
          AUCTION_ID,
          auction.preview_scheduled_at,
          auction.scheduled_open_at,
          end,
          1,
          prepareAdminAuctionScheduleOnce(
            auction.preview_scheduled_at,
            auction.scheduled_open_at,
            end,
            1,
          ),
          signal,
        ),
    ),
  );
  expect(result.state_version).toBe(2);
  expect(result.scheduled_ends_at).toBe("2025-11-15T13:46:40.110000Z");
});
