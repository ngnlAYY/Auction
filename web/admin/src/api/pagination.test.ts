// Admin API 分页回显的页码、页大小与计数边界回归。

import { describe, expect, test } from "bun:test";

import { validPageEcho } from "./pagination";
import { MAX_JSON_SAFE_INTEGER } from "./validation";

describe("Admin API 分页回显（对应 Rust api/pagination.rs）", () => {
  test("page_echo_accepts_matching_bounded_pages", () => {
    expect(validPageEcho(1, 20, 1, 20, 0, 0)).toBe(true);
    expect(validPageEcho(3, 20, 3, 20, 45, 5)).toBe(true);
    // 页码越过末页时服务端仍回显请求页，条数为 0。
    expect(validPageEcho(9, 20, 9, 20, 45, 0)).toBe(true);
  });

  test("page_echo_rejects_mismatch_and_overflowing_counts", () => {
    // 页码或页大小未原样回显。
    expect(validPageEcho(2, 20, 1, 20, 45, 20)).toBe(false);
    expect(validPageEcho(1, 20, 1, 50, 45, 20)).toBe(false);
    // 条数超过页大小或总数。
    expect(validPageEcho(1, 20, 1, 20, 45, 21)).toBe(false);
    expect(validPageEcho(1, 20, 1, 20, 3, 4)).toBe(false);
    // 三元组本身越界：页码/页大小非正、总数为负或越过 JSON 安全整数。
    expect(validPageEcho(0, 20, 0, 20, 0, 0)).toBe(false);
    expect(validPageEcho(1, 0, 1, 0, 0, 0)).toBe(false);
    expect(validPageEcho(1, 20, 1, 20, -1, 0)).toBe(false);
    expect(validPageEcho(1, 20, 1, 20, MAX_JSON_SAFE_INTEGER + 1, 0)).toBe(false);
  });
});
