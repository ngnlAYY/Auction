// 成交结果的本页展示分组、全局排序控件及精确中位数格式。

import type {
  PublicPriceHistoryEntryResponse,
  PublicPriceHistoryStatsResponse,
} from "@auction/contracts/dto/public/catalog/history";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 展示方式独立于后端查询；切换分组不会重新请求相同的成交页。 */
export function HistoryResultControls(props: {
  readonly sort: string;
  readonly grouping: string;
  readonly onSortChange: (sort: string) => void;
  readonly onGroupChange: (grouping: string) => void;
}): JSX.Element {
  return (
    <>
      <div class="public-history-view-controls">
        <label>
          <span>{t("history-sort-label")}</span>
          <select
            name="history_sort"
            value={props.sort}
            onChange={(event) => props.onSortChange(event.currentTarget.value)}
          >
            <option value="closed_desc">{t("history-sort-closed-desc")}</option>
            <option value="closed_asc">{t("history-sort-closed-asc")}</option>
            <option value="price_desc">{t("history-sort-price-desc")}</option>
            <option value="price_asc">{t("history-sort-price-asc")}</option>
          </select>
        </label>
        <label>
          <span>{t("history-group-label")}</span>
          <select
            name="history_group"
            value={props.grouping}
            onChange={(event) => props.onGroupChange(event.currentTarget.value)}
          >
            <option value="flat">{t("history-group-flat")}</option>
            <option value="equipment">{t("history-group-equipment")}</option>
            <option value="buyer">{t("history-group-buyer")}</option>
            <option value="seller">{t("history-group-seller")}</option>
          </select>
        </label>
      </div>
      <p class="public-history-results-scope">{t("history-results-scope")}</p>
    </>
  );
}

/** 分组只持有当前页的真实行；顺序沿用各组在全局排序页中的首次出现位置。 */
export interface HistoryGroup {
  readonly key: string;
  readonly label: string;
  readonly rows: readonly PublicPriceHistoryEntryResponse[];
}

/** 按本页已有字段收集分组，不补查其他分页或推算全结果组数。 */
export function historyGroups(
  rows: readonly PublicPriceHistoryEntryResponse[],
  grouping: string,
): readonly HistoryGroup[] {
  if (grouping !== "equipment" && grouping !== "buyer" && grouping !== "seller") {
    const first = rows[0];
    if (first === undefined) {
      return [];
    }
    // 平铺直接移交原页缓冲，不逐行搬运到新数组或创建无意义的分组索引。
    return [{ key: first.lot_id, label: "", rows }];
  }
  const positions = new Map<string, number>();
  const groups: { key: string; label: string; rows: PublicPriceHistoryEntryResponse[] }[] = [];
  for (const row of rows) {
    let label: string;
    if (grouping === "equipment") {
      label = row.equipment_name;
    } else if (grouping === "buyer") {
      label = row.buyer_name;
    } else {
      label = row.seller_name;
    }
    const existing = positions.get(label);
    if (existing !== undefined) {
      groups[existing]?.rows.push(row);
    } else {
      positions.set(label, groups.length);
      groups.push({ key: row.lot_id, label, rows: [row] });
    }
  }
  return groups;
}

/** 半个 Credit 直接按精确字符串展示，不能把 .5 拼到缩写后的 K/M/B 金额上。 */
export function historyMedianAmount(stats: PublicPriceHistoryStatsResponse): string {
  const exact = stats.median_exact;
  if (exact === null || exact === undefined) {
    return formatCreditAmount(stats.median);
  }
  if (exact.endsWith(".5")) {
    const whole = parseI64BigInt(exact.slice(0, -2));
    if (whole !== null) {
      const amount = `${formatCreditAmountFullExact(whole)}.5`;
      return t("history-credit-amount", { amount });
    }
  }
  const parsed = parseI64BigInt(exact);
  return parsed === null ? formatCreditAmount(stats.median) : formatCreditAmountExact(parsed);
}

/** 解析 Rust `i64::from_str` 的十进制语法；保留完整范围，展示时再按源码算法格式化。 */
function parseI64BigInt(value: string): bigint | null {
  let index = 0;
  let negative = false;
  const first = value[0];
  if (first === "+" || first === "-") {
    negative = first === "-";
    index = 1;
  }
  if (index >= value.length) {
    return null;
  }
  let magnitude = 0n;
  for (; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
    magnitude = magnitude * 10n + BigInt(code - 0x30);
  }
  const maximum = negative ? 9_223_372_036_854_775_808n : 9_223_372_036_854_775_807n;
  if (magnitude > maximum) {
    return null;
  }
  return negative ? -magnitude : magnitude;
}

/** 与共享 `formatCreditAmount` 相同，但金额使用完整 `i64` 精度。 */
function formatCreditAmountExact(amount: bigint): string {
  const sign = amount < 0n ? "-" : "";
  const absolute = amount < 0n ? -amount : amount;
  if (absolute >= 1_000_000_000n && absolute % 1_000_000n === 0n) {
    return formatScaledAmountExact(sign, absolute, 1_000_000_000n, "B");
  }
  if (absolute >= 1_000_000n && absolute % 1_000n === 0n) {
    return formatScaledAmountExact(sign, absolute, 1_000_000n, "M");
  }
  if (absolute % 1_000n === 0n) {
    return `${sign}${absolute / 1_000n}K`;
  }
  return `${sign}${formatThousandsExact(absolute)}c`;
}

/** 与共享 `formatCreditAmountFull` 相同，但金额使用完整 `i64` 精度。 */
function formatCreditAmountFullExact(amount: bigint): string {
  const absolute = amount < 0n ? -amount : amount;
  return `${amount < 0n ? "-" : ""}${formatThousandsExact(absolute)}`;
}

/** 按源码的三位小数规则格式化缩放金额。 */
function formatScaledAmountExact(
  sign: string,
  amount: bigint,
  scale: bigint,
  suffix: string,
): string {
  const whole = amount / scale;
  const fraction = ((amount % scale) / (scale / 1_000n))
    .toString()
    .padStart(3, "0")
    .replace(/0+$/u, "");
  return fraction === "" ? `${sign}${whole}${suffix}` : `${sign}${whole}.${fraction}${suffix}`;
}

/** 逐位加千分位，避免本地化分组分隔符随语言变化。 */
function formatThousandsExact(value: bigint): string {
  const digits = value.toString();
  let formatted = "";
  for (let index = 0; index < digits.length; index += 1) {
    if (index > 0 && (digits.length - index) % 3 === 0) {
      formatted += ",";
    }
    formatted += digits[index];
  }
  return formatted;
}
