// Public 查询对的顺序、默认值省略与透传参数。

import { expect, test } from "bun:test";

import type { PublicBoardQuery } from "./board";

const { normalizeWorldQuery, publicBoardQueryDefault } = await import("./board");
const { publicQueryPairs, publicQueryString } = await import("./serialization");

function pairs(values: readonly (readonly [string, string])[]): [string, string][] {
  return values.map(([key, value]) => [key, value]);
}

test("canonical_pairs_follow_the_documented_order_and_keep_passthrough：规范参数按序输出并保留透传", () => {
  const query: PublicBoardQuery = {
    ...publicBoardQueryDefault(),
    world: "main",
    category: "One-Handed",
    status: "live",
    historySort: "price_desc",
    historyGroup: "buyer",
    page: 2,
    passthrough: [["utm", "a b"]],
  };
  expect(publicQueryPairs(query)).toEqual(
    pairs([
      ["world", "main"],
      ["type", "One-Handed"],
      ["status", "live"],
      ["history_sort", "price_desc"],
      ["history_group", "buyer"],
      ["page", "2"],
      ["page_size", "20"],
      ["movement_page_size", "20"],
      ["utm", "a b"],
    ]),
  );

  const encoded = publicQueryString(query);
  expect(encoded.startsWith("?")).toBe(true);
  expect(encoded.includes("utm=a+b")).toBe(true);
  expect(encoded.includes("history_sort=closed_desc")).toBe(false);
});

test("omitted_defaults_are_not_written_back_and_invalid_world_values_are_dropped：默认值不写回且非法世界被丢弃", () => {
  const defaultPages = {
    ...publicBoardQueryDefault(),
    page: 1,
    movementPage: 1,
    bidPage: 1,
    proxyPage: 1,
  };
  expect(publicQueryPairs(defaultPages).some(([key]) => key === "page")).toBe(false);

  const query = { ...publicBoardQueryDefault(), includeUnleveled: "false" };
  expect(publicQueryPairs(query)).toContainEqual(["include_unleveled", "false"]);

  expect(normalizeWorldQuery(" main ")).toBe("main");
  expect(normalizeWorldQuery("isekai")).toBe("isekai");
  expect(normalizeWorldQuery("MAIN")).toBeNull();
  expect(normalizeWorldQuery("")).toBeNull();
});

test("passthrough_parameters_survive_round_trip_encode：透传参数原样编码且排在页面键之后", () => {
  const query: PublicBoardQuery = {
    ...publicBoardQueryDefault(),
    passthrough: [["ref", "a&b=c"]],
  };
  const encoded = publicQueryString(query);
  expect(encoded.includes("ref=a%26b%3Dc")).toBe(true);

  // 页面拥有的键排在外部透传键之前，外部键不覆盖 canonical 状态。
  const keys = publicQueryPairs(query);
  const pageSize = keys.findIndex(([key]) => key === "page_size");
  const passthrough = keys.findIndex(([key]) => key === "ref");
  // 与 Rust 的 position().unwrap() 一样，两个参数都必须存在；否则 -1 会让顺序断言误通过。
  expect(pageSize).toBeGreaterThanOrEqual(0);
  expect(passthrough).toBeGreaterThanOrEqual(0);
  expect(pageSize).toBeLessThan(passthrough);
});
