// Public 地址栏查询解析：保留待页面校验的 lot，并严格恢复非法页长。

import { expect, test } from "bun:test";

const { parsePublicQuery } = await import("./parse");
const { publicQueryString } = await import("./serialization");

test("场次参数接受规范 UUID，并保留非法值供页面拒绝", () => {
  const auction = "67e55044-10b1-426f-9247-bb680e5fe0c8";
  expect(parsePublicQuery("/", "")).toMatchObject({ auctionId: null, invalidAuction: null });
  for (const version of [1, 2, 3, 4, 5]) {
    const valid = auction.replace("426f", `${version}26f`);
    expect(parsePublicQuery("/", `?auction=${valid}`)).toMatchObject({
      auctionId: valid,
      invalidAuction: null,
    });
  }
  for (const invalid of [
    "",
    "truncated",
    auction.toUpperCase(),
    auction.replace("426f", "726f"),
    auction.replace("9247", "4247"),
    auction.replaceAll("-", ""),
    `{${auction}}`,
    `urn:uuid:${auction}`,
  ]) {
    const query = parsePublicQuery("/", `?auction=${invalid}`);
    expect(query).toMatchObject({ auctionId: null, invalidAuction: [invalid] });
    expect(new URLSearchParams(publicQueryString(query)).get("auction")).toBe(invalid);
  }
  const duplicate = parsePublicQuery("/", `?auction=${auction}&auction=${auction}`);
  expect(duplicate).toMatchObject({ auctionId: null, invalidAuction: [auction, auction] });
  expect(new URLSearchParams(publicQueryString(duplicate)).getAll("auction")).toEqual([
    auction,
    auction,
  ]);
});

test("查询只去掉分隔符，参数名开头的问号原样保留", () => {
  const query = parsePublicQuery("/", "??page=2&ref=x");
  expect(query.page).toBe(1);
  expect(query.passthrough).toEqual([
    ["?page", "2"],
    ["ref", "x"],
  ]);
  expect(parsePublicQuery("/", "?page=2").page).toBe(2);
});

test("invalid_lot_in_the_address_bar_is_kept_instead_of_widening_the_bid_log：地址栏非法 lot 保留而不放宽竞价日志", () => {
  // 超长或含控制字符的 lot 原样保留，竞价日志页据此报字段错误且不发请求。
  const longLot = "a".repeat(129);
  const query = parsePublicQuery("/bidlog", `?lot=${longLot}`);
  expect(query.bidLogLot).toBe(longLot);
  const control = parsePublicQuery("/bidlog", "?lot=One%0712");
  expect(control.bidLogLot).toBe("One\u000712");
  // 只去两端空白，空值仍表示不筛选；历史名称沿用同一口径。
  const padded = parsePublicQuery("/bidlog", "?lot=%20One12%20");
  expect(padded.bidLogLot).toBe("One12");
  expect(parsePublicQuery("/bidlog", "?lot=%20%09").bidLogLot).toBeNull();
  const history = parsePublicQuery("/history", "?name=+Phase+Robe+");
  expect(history.historyName).toBe("Phase Robe");
});

test("linked_page_sizes_above_the_public_maximum_restore_the_default：超过 Public 上限的链接页长恢复默认", () => {
  // 旧链接里超过契约上限的数量按非法值恢复默认，不原样发给必然拒绝它的接口。
  const atLimit = parsePublicQuery("/", "?page_size=200&movement_page_size=200");
  expect([atLimit.pageSize, atLimit.movementPageSize]).toEqual([200, 200]);
  const overLimit = parsePublicQuery("/", "?page_size=201&movement_page_size=1000");
  expect([overLimit.pageSize, overLimit.movementPageSize]).toEqual([20, 20]);
  const invalid = parsePublicQuery("/compensation", "?page_size=0&movement_page_size=-5");
  expect([invalid.pageSize, invalid.movementPageSize]).toEqual([20, 20]);
});

test("catalog_type_lists_are_normalized_and_encoded_as_one_comma_value：分类列表规范化并编码为单个逗号参数", () => {
  const single = parsePublicQuery("/", "?type=clo");
  expect(single.category).toBe("clo");
  expect(publicQueryString(single)).toContain("type=clo");

  const multiple = parsePublicQuery("/", "?type=sta,one");
  expect(multiple.category).toBe("one,sta");
  const encoded = publicQueryString(multiple);
  expect(encoded).toContain("type=one%2Csta");
  expect(encoded).not.toContain("type=one,sta");

  for (const invalid of ["one,one", "one,", "one,,sta", "one,all", "unknown"]) {
    const query = parsePublicQuery("/", `?type=${encodeURIComponent(invalid)}`);
    expect(query.category).toBeNull();
    expect(query.invalidCategory).toEqual([invalid]);
    expect(new URLSearchParams(publicQueryString(query)).getAll("type")).toEqual([invalid]);
  }
  const duplicate = parsePublicQuery("/", "?type=clo&type=sta");
  expect(duplicate.category).toBeNull();
  expect(duplicate.invalidCategory).toEqual(["clo", "sta"]);
});
