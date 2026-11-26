// 寄售查询只接受规格范围内的单值与规范文本；地址键与拍卖页同名。

import { expect, test } from "bun:test";

import { parseConsignmentQuery } from "./filters";

function parse(search: string) {
  return parseConsignmentQuery([...new URLSearchParams(search)]);
}

test("默认值独立于拍卖查询，合法偏移边界与表单字符原样往返", () => {
  expect(parse("world=main&page=4&page_size=10&bid_page=5&proxy_page=9")).toEqual({
    mode: null,
    category: "",
    q: "",
    page: 1,
    pageSize: 20,
  });
  expect(
    parse("mode=direct&type=sta,one&name=%25_%2B&consignment_page=51&consignment_page_size=200"),
  ).toEqual({
    mode: "direct",
    // 分类按规范顺序保存，与拍卖页的 type 一致。
    category: "one,sta",
    q: "%_+",
    page: 51,
    pageSize: 200,
  });
  expect(parse("consignment_page_size=1&consignment_page=10001")?.page).toBe(10001);
  const accepted = new URLSearchParams({ name: "🗡".repeat(80) });
  expect(parse(accepted.toString())?.q).toBe("🗡".repeat(80));
});

test.each([
  "mode=registered_only",
  "mode=direct&mode=direct",
  "type=Staff",
  "type=sta,sta",
  "type=",
  "name=",
  "name=staff%0Aaxe",
  "name=staff%E2%80%A8axe",
  "consignment_page=0",
  "consignment_page=9007199254740993",
  "consignment_page=52&consignment_page_size=200",
  "consignment_page_size=201",
  "consignment_page_size=1e2",
  new URLSearchParams({ name: "a".repeat(81) }).toString(),
])("拒绝非法寄售查询 %s", (search) => {
  expect(parse(search)).toBeNull();
});

test.each(["://", "/EqUiP/", "/EqUip?", "kEy="])("名称拒绝内嵌凭证片段 %s", (fragment) => {
  const query = new URLSearchParams({ name: `before${fragment}after` });
  expect(parse(query.toString())).toBeNull();
});
