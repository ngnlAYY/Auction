// 标记命令的成功解锁以包含其结果的整场重读为界。

import { expect, test } from "bun:test";
import type { PublicSetLotWatchRequest } from "@auction/contracts/dto/public/watch";

import type { CommandError } from "../../api/commands";
import type { WatchCommand } from "./commands";

const { settledLots } = await import(".");

function command(auctionId: string, awaitingRefresh: number | null): WatchCommand {
  const payload: PublicSetLotWatchRequest = {
    command_id: "command-1",
    watched: true,
    channel: "mm",
    expected_version: 3,
  };
  return {
    auctionId,
    lotCode: "Clo16",
    payload,
    busy: false,
    uncertain: false,
    error: null,
    awaitingRefresh,
  };
}

test("success_unlocks_only_after_a_collection_read_started_after_it：成功命令只在之后的整场读取完成后解锁", () => {
  const commands = new Map([["lot-1", command("auction-7", 4)]]);
  // 命令成功前发起的读取不含本次结果，lot 保持锁定。
  expect(settledLots(commands, "auction-7", 3)).toEqual([]);
  expect(settledLots(commands, "auction-7", 4)).toEqual(["lot-1"]);
  expect(settledLots(commands, "auction-7", 5)).toEqual(["lot-1"]);
});

test("pending_failed_and_other_auction_commands_stay_locked：忙碌、失败与其他场次的命令保持锁定", () => {
  const busy = { ...command("auction-7", null), busy: true };
  const uncertain = {
    ...command("auction-7", null),
    uncertain: true,
    error: { code: "network_error", status: 0 } satisfies CommandError,
  };
  const commands = new Map([
    ["lot-busy", busy],
    ["lot-uncertain", uncertain],
    ["lot-other", command("auction-8", 1)],
  ]);
  expect(settledLots(commands, "auction-7", 9)).toEqual([]);
});
