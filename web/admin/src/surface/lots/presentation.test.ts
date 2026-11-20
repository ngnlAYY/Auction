// 对照 Rust 拍品编号和起拍价边界。

import { expect, test } from "bun:test";
import { adminLotCodeFor, parseStartPrice } from "./presentation";

test("lot_codes_match_forum_prefixes_and_fall_back_to_lot", () => {
  expect(adminLotCodeFor("Two-Handed", 7)).toBe("Two07");
  expect(adminLotCodeFor("Shield", 12)).toBe("Shd12");
  expect(adminLotCodeFor("Unknown", 1)).toBe("Lot01");
});

test("new_start_prices_stay_within_the_bid_range", () => {
  expect(parseStartPrice("1")).toBe(1);
  expect(parseStartPrice("1000000000")).toBe(1_000_000_000);
  for (const input of ["0", "1000000001", "-1", "1.5", "99999999999999999999"]) {
    expect(parseStartPrice(input)).toBeNull();
  }
});
