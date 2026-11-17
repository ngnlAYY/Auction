// 竞价日志 lot 筛选接受与拒绝的取值集合。

import { expect, test } from "bun:test";

const { validLotFilter } = await import("./lots");

test("lot_filter_accepts_what_the_bid_log_endpoint_accepts：lot 筛选接受接口接受的取值", () => {
  expect(validLotFilter("")).toBe(true);
  expect(validLotFilter("One12")).toBe(true);
  expect(validLotFilter("a".repeat(32))).toBe(true);
  expect(validLotFilter("67e55044-10b1-426f-9247-bb680e5fe0c8")).toBe(true);
});

test("lot_filter_rejects_values_the_endpoint_answers_with_400：lot 筛选拒绝接口会以 400 拒绝的取值", () => {
  expect(validLotFilter("a".repeat(33))).toBe(false);
  expect(validLotFilter("One 12")).toBe(false);
  expect(validLotFilter("One-12")).toBe(false);
  expect(validLotFilter("拍品12")).toBe(false);
  // 地址栏原样保留的超长或含控制字符的值同样落到字段错误，不发请求。
  expect(validLotFilter("a".repeat(129))).toBe(false);
  expect(validLotFilter("One\u000712")).toBe(false);
  // 服务端只接受规范小写连字符形式的非 nil UUID。
  expect(validLotFilter("67E55044-10B1-426F-9247-BB680E5FE0C8")).toBe(false);
  expect(validLotFilter("{67e55044-10b1-426f-9247-bb680e5fe0c8}")).toBe(false);
  expect(validLotFilter("00000000-0000-0000-0000-000000000000")).toBe(false);
});
