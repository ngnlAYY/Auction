// 页头切换世界时查询状态的保留与历史看板回首页行为。

import { expect, test } from "bun:test";

import type { PublicBoardQuery } from "../query/board";

const { publicBoardQueryDefault } = await import("../query/board");
const { worldSwitchQuery } = await import("./world");

function historicalQuery(): PublicBoardQuery {
  return {
    ...publicBoardQueryDefault(),
    auctionId: "67e55044-10b1-426f-9247-bb680e5fe0c8",
    category: "clo",
    equipmentGroup: "rare",
    maxLevel: "400",
    page: 3,
    pageSize: 50,
  };
}

test("switching_world_on_a_historical_board_returns_to_the_world_home：历史看板切换世界会回到目标世界首页", () => {
  const query = worldSwitchQuery(historicalQuery(), "isekai", true);
  // 场次、筛选与页码全部清除，每页条数取本地偏好。
  expect(query).toEqual({ ...publicBoardQueryDefault(), world: "isekai" });
  expect(worldSwitchQuery(historicalQuery(), "main", true)).toEqual(publicBoardQueryDefault());
});

test("switching_world_elsewhere_only_changes_the_world：其他页面切换世界只改世界", () => {
  const current = historicalQuery();
  const query = worldSwitchQuery(current, "isekai", false);
  expect(query).toEqual({ ...current, world: "isekai" });
  const currentIsekai = { ...historicalQuery(), world: "isekai" };
  expect(worldSwitchQuery(currentIsekai, "main", false).world).toBeNull();
});
