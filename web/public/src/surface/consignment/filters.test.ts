// 寄售查询只接受规格范围内的单值与规范文本。

import { expect, test } from "bun:test";

import { parseConsignmentQuery } from "./filters";

function parse(search: string) {
  return parseConsignmentQuery([...new URLSearchParams(search)]);
}

test("默认值独立于拍卖查询，合法偏移边界与表单字符原样往返", () => {
  expect(parse("world=main&page=4&page_size=10&bid_page=5&proxy_page=9")).toEqual({
    world: null,
    mode: null,
    category: "",
    q: "",
    page: 1,
    pageSize: 20,
  });
  expect(
    parse(
      "consignment_world=isekai&consignment_mode=direct&consignment_category=%E5%89%91&consignment_q=%25_%2B&consignment_page=51&consignment_page_size=200",
    ),
  ).toEqual({
    world: "isekai",
    mode: "direct",
    category: "剑",
    q: "%_+",
    page: 51,
    pageSize: 200,
  });
  expect(parse("consignment_page_size=1&consignment_page=10001")?.page).toBe(10001);
  const accepted = new URLSearchParams({
    consignment_category: "🗡".repeat(32),
    consignment_q: "🗡".repeat(80),
  });
  expect(parse(accepted.toString())?.q).toBe("🗡".repeat(80));
});

test.each([
  "consignment_world=unknown",
  "consignment_world=main&consignment_world=main",
  "consignment_mode=registered_only",
  "consignment_category=%20Staff",
  "consignment_q=",
  "consignment_q=staff%0Aaxe",
  "consignment_q=staff%E2%80%A8axe",
  "consignment_page=0",
  "consignment_page=9007199254740993",
  "consignment_page=52&consignment_page_size=200",
  "consignment_page_size=201",
  "consignment_page_size=1e2",
  new URLSearchParams({ consignment_category: "🗡".repeat(33) }).toString(),
  new URLSearchParams({ consignment_q: "a".repeat(81) }).toString(),
])("拒绝非法寄售查询 %s", (search) => {
  expect(parse(search)).toBeNull();
});

test.each(["://", "/EqUiP/", "/EqUip?", "kEy="])("名称与分类均拒绝内嵌凭证片段 %s", (fragment) => {
  for (const key of ["consignment_q", "consignment_category"]) {
    const query = new URLSearchParams({ [key]: `before${fragment}after` });
    expect(parse(query.toString()) === null).toBe(true);
  }
});
