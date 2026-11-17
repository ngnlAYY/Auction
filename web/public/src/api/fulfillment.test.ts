// 履约进度缓存确认时刻格式的回归断言。

import { expect, test } from "bun:test";

const { confirmationTime } = await import("./fulfillment");

test("confirmation_time_is_whole_seconds_in_utc：确认时刻是 UTC 整秒", () => {
  expect(confirmationTime(1_763_200_000_750)).toBe("2025-11-15T09:46:40Z");
  expect(confirmationTime(0)).toBe("1970-01-01T00:00:00Z");
  // 超出可表示范围时不猜测时刻，由调用方按读取失败处理。
  // 已知差异：u64::MAX 与 i64::MAX 超过 2^53−1，JS 无法精确表示；用可表示的最大安全整数
  // 验证同样的越界失败。
  expect(confirmationTime(Number.MAX_SAFE_INTEGER)).toBeNull();
  expect(confirmationTime(Number.MAX_SAFE_INTEGER - 1)).toBeNull();
});
