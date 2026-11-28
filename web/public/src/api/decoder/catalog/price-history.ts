// 价格历史编号页、行字段闭集与精确中位数字段校验。

import { JSON_MAX_SAFE_INTEGER, type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  accountNonempty,
  int,
  object,
  optionalNonnegative,
  positive,
  string,
  time,
} from "..";
import { validNumberedPage } from "./auction";

/** 校验价格历史编号页、逐行字段与统计形状。 */
export function validPriceHistoryPage(value: unknown): boolean {
  const page = object(value);
  if (page === null) return false;
  const rows = page.rows;
  if (!jsonKeysOnly(page, ["page", "page_size", "rows", "stats", "total"], [])) return false;
  if (!validNumberedPage(page) || !Array.isArray(rows)) return false;
  return rows.every(validPriceHistoryEntry) && validPriceHistoryStats(page.stats);
}

function validPriceHistoryEntry(value: unknown): boolean {
  const entry = object(value);
  if (entry === null) return false;
  return (
    jsonKeysOnly(
      entry,
      [
        "lot_id",
        "auction_id",
        "world_id",
        "auction_sequence",
        "lot_number",
        "equipment_name",
        "equipment_category",
        "price",
        "buyer_name",
        "seller_name",
        "closed_at",
      ],
      ["equipment_level"],
    ) &&
    validUuid(string(entry.lot_id) ?? "") &&
    validUuid(string(entry.auction_id) ?? "") &&
    accountNonempty(entry.world_id) &&
    positive(entry.auction_sequence) &&
    positive(entry.lot_number) &&
    accountNonempty(entry.equipment_name) &&
    accountNonempty(entry.equipment_category) &&
    optionalNonnegative(entry, "equipment_level") &&
    positive(entry.price) &&
    accountNonempty(entry.buyer_name) &&
    accountNonempty(entry.seller_name) &&
    time(entry.closed_at)
  );
}

function validPriceHistoryStats(value: unknown): boolean {
  const stats = object(value);
  if (stats === null) return false;
  if (!jsonKeysOnly(stats, ["count", "min", "max", "median"], ["median_exact"])) return false;
  const count = int(stats.count);
  const minimum = int(stats.min);
  const maximum = int(stats.max);
  const median = int(stats.median);
  if (count === null || minimum === null || maximum === null || median === null) return false;
  // 与服务端统计校验一致：空集合三项全零且无精确中位数，非空集合满足 min ≤ median ≤ max。
  const shape =
    count === 0
      ? minimum === 0 &&
        maximum === 0 &&
        median === 0 &&
        (stats.median_exact === undefined || stats.median_exact === null)
      : minimum > 0 && maximum >= minimum && median >= minimum && median <= maximum;
  return shape && validExactHistoryMedian(stats);
}

// 精确字段只接受规范整数或 .5；以二倍整数核对范围，并保留旧 median 的 ties-to-even 口径。
function validExactHistoryMedian(value: JsonObject): boolean {
  const exact = value.median_exact;
  if (exact === undefined || exact === null) return true;
  if (typeof exact !== "string") return false;
  const dot = exact.indexOf(".");
  const wholeText = dot < 0 ? exact : exact.slice(0, dot);
  const half = dot >= 0;
  if (half && exact.slice(dot + 1) !== "5") return false;
  // 单次线性扫描，不为攻击者控制的长度分配逐字符数组。
  if (
    wholeText.length === 0 ||
    (wholeText.length > 1 && wholeText.startsWith("0")) ||
    !/^[0-9]+$/u.test(wholeText)
  ) {
    return false;
  }
  const minimum = int(value.min);
  const maximum = int(value.max);
  const median = int(value.median);
  if (minimum === null || maximum === null || median === null) return false;
  const whole = BigInt(wholeText);
  const doubled = whole * 2n + (half ? 1n : 0n);
  const rounded = whole + (half && whole % 2n === 1n ? 1n : 0n);
  const maximumSafe = BigInt(JSON_MAX_SAFE_INTEGER);
  return (
    doubled <= maximumSafe * 2n &&
    doubled >= BigInt(minimum) * 2n &&
    doubled <= BigInt(maximum) * 2n &&
    rounded === BigInt(median)
  );
}
