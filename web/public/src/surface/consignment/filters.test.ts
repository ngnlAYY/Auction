// 寄售查询只接受规格范围内的单值与规范文本；地址键与拍卖页同名。

import { expect, test } from "bun:test";

import { consignmentCatalogParams } from "../../api/consignment-catalog";
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
  // 整串 80 个字符；按空白拆出的每段不超过 128 字节（"🗡" 占 4 字节）。
  const longest = `${"🗡".repeat(32)} ${"🗡".repeat(32)} ${"🗡".repeat(14)}`;
  expect(parse(new URLSearchParams({ name: longest }).toString())?.q).toBe(longest);
});

test("名称按成交历史页口径分词：地址栏与目录请求原样往返", () => {
  const query = parse("name=Leg+Kat%2CSta");
  expect(query?.q).toBe("Leg Kat,Sta");
  if (query === null) throw new Error("合法名称被拒绝");
  expect(consignmentCatalogParams({ ...query, world: null }).get("q")).toBe("Leg Kat,Sta");
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
  // 单段超过 128 字节、只有分隔符、超过 8 个片段，都与服务端一致失败关闭。
  new URLSearchParams({ name: "🗡".repeat(33) }).toString(),
  "name=%2C",
  "name=a+b+c+d+e+f+g+h+i",
])("拒绝非法寄售查询 %s", (search) => {
  expect(parse(search)).toBeNull();
});

test.each(["://", "/EqUiP/", "/EqUip?", "kEy="])("名称拒绝内嵌凭证片段 %s", (fragment) => {
  const query = new URLSearchParams({ name: `before${fragment}after` });
  expect(parse(query.toString())).toBeNull();
});
