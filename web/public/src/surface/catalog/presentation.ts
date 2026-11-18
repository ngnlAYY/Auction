// Public 拍卖目录的分类、状态、链接、金额和倒计时映射。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { useSecondTick } from "@auction/web-shared/platform/clock";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { utf8ByteLength } from "@auction/web-shared/text";
import { currentLanguage, formatLocalizedTimeForLanguage, t } from "@auction/web-shared/ui/i18n";
import type { Accessor } from "solid-js";

import { estimatedNow } from "../../api/clock";
import type { CatalogRow } from "../../api/model";
import { parseStrictRfc3339 } from "../../api/session/rfc3339";
import {
  auctionParameter,
  type PublicBoardQuery,
  publicBoardQueryDefault,
  publicHref,
} from "../query";
import { rustTrim } from "../query/text";

const UTF8_ENCODER = new TextEncoder();

/** 统计某个分类的拍品数；服务端计数优先，缺失时按行分类回退，`all` 使用全部行数。 */
export function catalogCount(
  counts: ReadonlyMap<string, number>,
  category: string,
  rows: readonly CatalogRow[],
): number {
  const stored = counts.get(category);
  if (stored !== undefined) {
    return stored;
  }
  if (category === "all") {
    return rows.length;
  }
  return rows.filter((row) => catalogCategoryKey(row.category) === category).length;
}

/** 把公开键、规范大类名或其全小写形式归一为公开键；其余分类归入 `other`。 */
export function catalogCategoryKey(category: string): string {
  const categoryBytes = UTF8_ENCODER.encode(category);
  const matched = EQUIPMENT_CATEGORY_CODES.find(([key, name]) => {
    return (
      key === category ||
      name === category ||
      (utf8ByteLength(name) === categoryBytes.length &&
        asciiLowercaseBytesEqual(name, categoryBytes))
    );
  });
  return matched?.[0] ?? "other";
}

function asciiLowercaseBytesEqual(name: string, input: Uint8Array): boolean {
  const nameBytes = UTF8_ENCODER.encode(name);
  if (nameBytes.length !== input.length) {
    return false;
  }
  for (const [index, nameByte] of nameBytes.entries()) {
    const lowered = nameByte >= 0x41 && nameByte <= 0x5a ? nameByte + 0x20 : nameByte;
    if (lowered !== input[index]) {
      return false;
    }
  }
  return true;
}

/** 把归一后的公开分类键翻译为当前语言文案；未知键显示 `category-other`。 */
export function catalogCategoryLabel(category: string): string {
  switch (category) {
    case "one":
      return t("category-one-handed");
    case "two":
      return t("category-two-handed");
    case "sta":
      return t("category-staff");
    case "shd":
      return t("category-shield");
    case "clo":
      return t("category-cloth");
    case "lig":
      return t("category-light");
    case "hea":
      return t("category-heavy");
    default:
      return t("category-other");
  }
}

/** 稀有与融合独立于装备品质和类型；此处仅翻译服务端给出的分组。 */
export function equipmentGroupLabel(equipmentGroup: string): string {
  switch (equipmentGroup) {
    case "rare":
      return t("equipment-group-rare");
    case "fusion":
      return t("equipment-group-fusion");
    default:
      return t("value-empty");
  }
}

/** 拍品状态文案；未知状态原样显示。 */
export function lotStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return t("status-pending");
    case "live":
      return t("status-live");
    case "sold":
      return t("status-sold");
    case "unsold":
      return t("status-unsold");
    default:
      return status;
  }
}

/** 拍品状态对应的 CSS 修饰后缀；未知状态归入 `unknown`。 */
export function lotStatusClass(status: string): string {
  switch (status) {
    case "pending":
      return "pending";
    case "live":
      return "live";
    case "sold":
      return "sold";
    case "unsold":
      return "unsold";
    default:
      return "unknown";
  }
}

/** 场次状态文案；未知状态原样显示。 */
export function auctionStatusLabel(status: string): string {
  switch (status) {
    case "draft":
      return t("auction-status-draft");
    case "live":
      return t("auction-status-live");
    case "preview":
      return t("auction-status-preview");
    case "final":
      return t("auction-status-final");
    case "settling":
      return t("auction-status-settling");
    case "settled":
      return t("auction-status-settled");
    case "archived":
      return t("auction-status-archived");
    case "cancelled":
      return t("auction-status-cancelled");
    case "sold":
      return t("status-ended");
    default:
      return status;
  }
}

/** 场次状态对应的 CSS 修饰后缀；未知状态归入 `unknown`。 */
export function auctionStatusClass(status: string): string {
  switch (status) {
    case "draft":
      return "draft";
    case "live":
      return "live";
    case "preview":
      return "preview";
    case "final":
      return "final";
    case "settling":
      return "settling";
    case "cancelled":
      return "cancelled";
    case "sold":
      return "sold";
    case "settled":
      return "settled";
    default:
      return "unknown";
  }
}

/** 历史目录的场次状态文案：live/final/settled 使用历史措辞，其余沿用普通场次文案。 */
export function historicalAuctionStatusLabel(status: string): string {
  switch (status) {
    case "live":
      return t("auction-history-status-live");
    case "final":
      return t("auction-history-status-final");
    case "settled":
      return t("auction-history-status-settled");
    default:
      return auctionStatusLabel(status);
  }
}

/** 历史场次拍品目录入口；主世界省略 `world` 查询参数。 */
export function historicalLotsHref(auctionId: string, worldId: string): string {
  const query: PublicBoardQuery = {
    ...publicBoardQueryDefault(),
    auction: auctionParameter(auctionId),
    world: worldId === "main" ? null : worldId,
  };
  return publicHref("/", query);
}

/** 历史场次出价记录入口；`world` 是当前地址栏要继承的世界范围，`null` 表示全部世界。
 * 目标页是跨世界页，钻取链接不能替用户改范围，因此不能使用场次自己的 `world_id`。 */
export function historicalBidLogHref(auctionId: string, world: string | null): string {
  const query: PublicBoardQuery = {
    ...publicBoardQueryDefault(),
    auction: auctionParameter(auctionId),
    world,
  };
  return publicHref("/bidlog", query);
}

/** 已结算场次的履约进度入口；`world` 是当前地址栏要继承的世界范围，`null` 表示全部世界。
 * 目标页是跨世界页，钻取链接不能替用户改范围，因此不能使用场次自己的 `world_id`。 */
export function historicalFulfillmentHref(auctionId: string, world: string | null): string {
  const query: PublicBoardQuery = {
    ...publicBoardQueryDefault(),
    auction: auctionParameter(auctionId),
    world,
  };
  return publicHref("/fulfillment", query);
}

/** 按当前语言格式化拍卖时间文本。 */
export function formatAuctionTime(value: string): string {
  return formatLocalizedTimeForLanguage(value, currentLanguage());
}

/** 解析 credit 缩写；无后缀或 K 使用整数千，M/B 最多支持三/六位小数，精确到信用网格。
 * 浏览器只用该结果做即时提示，服务端仍会再次校验金额、拍卖状态和竞价资格。 */
export function parseCreditAmount(input: string): number | null {
  const trimmed = rustTrim(input);
  if (trimmed === "") {
    return null;
  }
  let numberText: string;
  let scale: number;
  let maximumDecimals: number;
  switch (trimmed.charAt(trimmed.length - 1)) {
    case "k":
    case "K":
      numberText = trimmed.slice(0, -1);
      scale = 1_000;
      maximumDecimals = 0;
      break;
    case "m":
    case "M":
      numberText = trimmed.slice(0, -1);
      scale = 1_000_000;
      maximumDecimals = 3;
      break;
    case "b":
    case "B":
      numberText = trimmed.slice(0, -1);
      scale = 1_000_000_000;
      maximumDecimals = 6;
      break;
    default:
      numberText = trimmed;
      scale = 1_000;
      maximumDecimals = 0;
      break;
  }
  const dot = numberText.indexOf(".");
  const whole = dot === -1 ? numberText : numberText.slice(0, dot);
  const fraction = dot === -1 ? null : numberText.slice(dot + 1);
  if (whole === "" || !asciiDigits(whole)) {
    return null;
  }
  if (
    fraction !== null &&
    (fraction === "" || utf8ByteLength(fraction) > maximumDecimals || !asciiDigits(fraction))
  ) {
    return null;
  }
  // 用 BigInt 复现源码的 checked i64 运算与最终上限判断；任一超出范围的分支都失败关闭。
  const wholeValue = BigInt(whole);
  const fractionalCredits =
    fraction === null ? 0n : BigInt(fraction) * (BigInt(scale) / 10n ** BigInt(fraction.length));
  const amount = wholeValue * BigInt(scale) + fractionalCredits;
  if (amount <= 0n || amount > BigInt(MAX_PUBLIC_BID_AMOUNT)) {
    return null;
  }
  return Number(amount);
}

function asciiDigits(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 倒计时展示投影：`empty` 表示截止时间缺失或无法解析，`urgent` 表示剩余不超过 15 分钟。 */
export interface CountdownDisplay {
  readonly label: string;
  readonly urgent: boolean;
  readonly expired: boolean;
  readonly empty: boolean;
}

/** 截止文本变化时才重新解析；秒针和语言变化仍即时更新显示，不缓存服务端校时结果。 */
export function useCountdownDisplay(endsAt: Accessor<string>): Accessor<CountdownDisplay> {
  const end = createValueMemo(() => parseStrictRfc3339(endsAt()));
  const tick = useSecondTick();
  return createValueMemo(() => {
    tick();
    return countdownDisplay(end(), estimatedNow());
  });
}

function countdownDisplay(end: bigint | null, now: number): CountdownDisplay {
  if (end === null) {
    return {
      label: t("value-empty"),
      urgent: false,
      expired: false,
      empty: true,
    };
  }
  const nowNanoseconds = BigInt(now) * 1_000_000n;
  const remainingSeconds = Number((end - nowNanoseconds) / 1_000_000_000n);
  if (remainingSeconds <= 0) {
    return {
      label: t("auction-countdown-expired"),
      urgent: false,
      expired: true,
      empty: false,
    };
  }
  const days = Math.trunc(remainingSeconds / 86_400);
  const hours = Math.trunc((remainingSeconds % 86_400) / 3_600);
  const minutes = Math.trunc((remainingSeconds % 3_600) / 60);
  const seconds = remainingSeconds % 60;
  const clock = `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  return {
    label: days > 0 ? t("auction-countdown-days", { days, clock }) : clock,
    urgent: remainingSeconds <= 15 * 60,
    expired: false,
    empty: false,
  };
}

/** 倒计时样式：空、临期、过期与正常四态。 */
export function countdownClass(countdown: CountdownDisplay): string {
  if (countdown.empty) {
    return "public-auction-countdown public-auction-countdown--none";
  }
  if (countdown.urgent) {
    return "public-auction-countdown public-auction-countdown--closing";
  }
  if (countdown.expired) {
    return "public-auction-countdown public-auction-countdown--expired";
  }
  return "public-auction-countdown";
}

/** Enter 或 Space：按钮与可聚焦行的键盘激活键。 */
export function isActivationKey(key: string): boolean {
  return key === "Enter" || key === " ";
}
