// Public 查询状态的 URL 序列化与受管键过滤。

import type { PublicBoardQuery } from "./board";

/** 本模块管理的全部公开 query 键；`bidlog_page` 别名只为读取迁移期旧链接保留，写回时会被清除。 */
export const PUBLIC_QUERY_KEYS: readonly string[] = [
  "world",
  "auction",
  "type",
  "equipment_group",
  "max_level",
  "include_unleveled",
  "status",
  "name",
  "from",
  "to",
  "min_price",
  "max_price",
  "buyer",
  "seller",
  "history_sort",
  "history_group",
  "lot",
  "page",
  "page_size",
  "movement_page",
  "movement_page_size",
  "bidlog_page",
  "bid_page",
  "proxy_page",
];

export function publicQueryString(query: PublicBoardQuery): string {
  const pairs = publicQueryPairs(query);
  if (pairs.length === 0) {
    return "";
  }
  return `?${queryPairsString(pairs)}`;
}

/** 与 Rust `form_urlencoded::Serializer` 的默认字符集一致：空格写成 `+`，`* - . 0-9 A-Z _ a-z`
 * 以外的字节按 UTF-8 百分号编码；`~` 也编码为 `%7E`。 */
export function queryPairsString(pairs: readonly (readonly [string, string])[]): string {
  return pairs.map(([key, value]) => `${queryComponent(key)}=${queryComponent(value)}`).join("&");
}

function queryComponent(value: string): string {
  const bytes = new TextEncoder().encode(value);
  let encoded = "";
  for (const byte of bytes) {
    if (byte === 0x20) {
      encoded += "+";
    } else if (
      (byte >= 0x30 && byte <= 0x39) ||
      (byte >= 0x41 && byte <= 0x5a) ||
      (byte >= 0x61 && byte <= 0x7a) ||
      byte === 0x2a ||
      byte === 0x2d ||
      byte === 0x2e ||
      byte === 0x5f
    ) {
      encoded += String.fromCharCode(byte);
    } else {
      encoded += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return encoded;
}

/** 序列化公开端查询状态；数量即使为 10 也显式保留，防止本地偏好改变分享及历史语义。 */
export function publicQueryPairs(query: PublicBoardQuery): [string, string][] {
  const pairs: [string, string][] = [];
  if (query.world !== null) {
    pairs.push(["world", query.world]);
  }
  if (query.auction.kind === "selected") {
    pairs.push(["auction", query.auction.id]);
  } else if (query.auction.kind === "invalid") {
    for (const value of query.auction.values) {
      pairs.push(["auction", value]);
    }
  }
  if (query.equipmentGroup !== null) {
    pairs.push(["equipment_group", query.equipmentGroup]);
  }
  if (query.maxLevel !== null) {
    pairs.push(["max_level", query.maxLevel]);
  }
  if (query.includeUnleveled !== "true") {
    pairs.push(["include_unleveled", query.includeUnleveled]);
  }
  if (query.category !== null) {
    pairs.push(["type", query.category]);
  } else if (query.invalidCategory !== null) {
    for (const value of query.invalidCategory) {
      pairs.push(["type", value]);
    }
  }
  if (query.status !== null) {
    pairs.push(["status", query.status]);
  }
  const retained: readonly [string, string | null][] = [
    ["name", query.historyName],
    ["from", query.historyFrom],
    ["to", query.historyTo],
    ["min_price", query.historyMinPrice],
    ["max_price", query.historyMaxPrice],
    ["buyer", query.historyBuyer],
    ["seller", query.historySeller],
    ["lot", query.bidLogLot],
  ];
  for (const [key, value] of retained) {
    if (value !== null) {
      pairs.push([key, value]);
    }
  }
  if (query.historySort !== "closed_desc") {
    pairs.push(["history_sort", query.historySort]);
  }
  if (query.historyGroup !== "equipment") {
    pairs.push(["history_group", query.historyGroup]);
  }
  if (query.page > 1) {
    pairs.push(["page", query.page.toString()]);
  }
  pairs.push(["page_size", query.pageSize.toString()]);
  if (query.movementPage > 1) {
    pairs.push(["movement_page", query.movementPage.toString()]);
  }
  pairs.push(["movement_page_size", query.movementPageSize.toString()]);
  if (query.bidPage > 1) {
    pairs.push(["bid_page", query.bidPage.toString()]);
  }
  if (query.proxyPage > 1) {
    pairs.push(["proxy_page", query.proxyPage.toString()]);
  }
  for (const pair of query.passthrough) {
    pairs.push(pair);
  }
  return pairs;
}

/** 原地写回时未知参数由共享层保留一次；完整链接仍通过 publicQueryPairs 携带 passthrough。 */
export function managedPublicQueryPairs(query: PublicBoardQuery): [string, string][] {
  return publicQueryPairs(query).filter(([key]) => PUBLIC_QUERY_KEYS.includes(key));
}
