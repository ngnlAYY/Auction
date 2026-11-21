// 履约进度路由的 data 校验：字段闭集、状态闭集，以及按状态成对或条件出现的可选字段。
//
// 条件与服务端判定一一对应：流拍状态没有成交事实与退回次数；成交状态必有买家与成交价；付款
// 期限只随三种付款等待状态出现，且是与目标无关的严格 UTC 秒格式；退回次数与上限成对出现且
// 计数不超过上限，两种首次付款状态（待寄出 COD、等待确认收款）不带退回次数；非空页按请求页码
// 算出的 offset 加本页行数不超过总数。浏览器只校验这些形状，不重建状态判定。

import {
  type PublicFulfillmentPageResponse,
  PublicFulfillmentPageResponseSchema,
} from "@auction/contracts/dto/public/catalog/fulfillment";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import type { Result } from "@auction/web-shared/result";
import { compareText, utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { accountNonempty, decodeSuccess, int, object, positive, string } from ".";

/** 履约进度状态闭集；顺序与 docs/contracts/routes.md 的状态表一致。 */
export const FULFILLMENT_STATUSES: readonly string[] = [
  "unsold_return_pending",
  "unsold_return_sent",
  "unsold_returned",
  "unsold_return_on_hold",
  "payment_mail_pending",
  "awaiting_payment",
  "payment_overdue",
  "payment_on_hold",
  "returned_by_buyer",
  "returned_on_hold",
  "cod_resend_pending",
  "cod_resent_awaiting_payment",
  "cod_return_limit_reached",
  "handoff_pending",
  "handoff_sent",
  "handoff_on_hold",
  "handoff_returned_by_buyer",
  "payout_pending",
  "payout_sent",
  "payout_on_hold",
  "completed",
  "equipment_lost",
  "voided_return_pending",
  "voided_return_sent",
  "voided_returned",
  "voided_on_hold",
  "voided_refund_pending",
  "voided_relisted",
  "voided_relist_pending",
  "voided_relist_unavailable",
  "unknown",
];

/** 解码履约进度编号页，并把响应绑定到请求的场次、世界与分页。 */
export function decodeFulfillmentPage(
  response: TransportResponse,
  auctionId: string,
  world: string,
  page: number,
  pageSize: number,
): Result<PublicFulfillmentPageResponse, string> {
  return decodeSuccess(response, PublicFulfillmentPageResponseSchema, (value) =>
    validFulfillmentPage(value, auctionId, world, page, pageSize),
  );
}

/** 仅供测试使用：直接校验履约页的字段闭集、身份绑定与跨字段约束。 */
export function validFulfillmentPage(
  value: unknown,
  auctionId: string,
  world: string,
  page: number,
  pageSize: number,
): boolean {
  if (!Number.isSafeInteger(page) || page < 1 || !Number.isSafeInteger(pageSize) || pageSize < 1) {
    return false;
  }
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const total = int(decoded.total);
  if (total === null || total < 0) {
    return false;
  }
  if (
    !jsonKeysOnly(decoded, ["auction", "lots", "page", "page_size", "total"], []) ||
    int(decoded.page) !== page ||
    int(decoded.page_size) !== pageSize ||
    !validAuction(decoded.auction, auctionId, world)
  ) {
    return false;
  }
  const lots = decoded.lots;
  if (!Array.isArray(lots)) {
    return false;
  }
  const count = lots.length;
  if (count > pageSize || (count !== 0 && !pageWithinTotal(page, pageSize, count, total))) {
    return false;
  }
  if (!lots.every(validLot)) {
    return false;
  }
  const lotIds = new Set<string>();
  for (const value of lots) {
    const lotId = object(value)?.lot_id;
    if (typeof lotId !== "string" || lotIds.has(lotId)) {
      return false;
    }
    lotIds.add(lotId);
  }
  return true;
}

/** 非空页的末行序号不超过总数：按请求页码与页大小算出的 offset 加本页行数不得超过 `total`。 */
// 越过末页的空页仍然有效（筛选后总数变少或旧书签），由分页器纠偏页码。
function pageWithinTotal(page: number, pageSize: number, count: number, total: number): boolean {
  const previousPages = page - 1;
  if (!Number.isSafeInteger(previousPages) || previousPages < 0) {
    return false;
  }
  const offset = previousPages * pageSize;
  if (!Number.isSafeInteger(offset)) {
    return false;
  }
  const end = offset + count;
  if (!Number.isSafeInteger(end)) {
    return false;
  }
  return end <= total;
}

function validAuction(value: unknown, auctionId: string, world: string): boolean {
  const decoded = object(value);
  return (
    decoded !== null &&
    jsonKeysOnly(decoded, ["id", "world", "number"], []) &&
    string(decoded.id) === auctionId &&
    validUuid(auctionId) &&
    string(decoded.world) === world &&
    positive(decoded.number)
  );
}

/** 仅供测试使用：直接校验单条履约 lot 的状态与可选字段组合。 */
export function validLot(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) {
    return false;
  }
  const status = string(decoded.status);
  if (status === null || !FULFILLMENT_STATUSES.includes(status)) {
    return false;
  }
  return (
    jsonKeysOnly(
      decoded,
      ["lot_id", "lot_number", "equipment_name", "equipment_category", "seller_name", "status"],
      ["buyer_name", "final_price", "payment_deadline_at", "cod_return_count", "cod_return_limit"],
    ) &&
    validUuid(string(decoded.lot_id) ?? "") &&
    positive(decoded.lot_number) &&
    accountNonempty(decoded.equipment_name) &&
    accountNonempty(decoded.equipment_category) &&
    accountNonempty(decoded.seller_name) &&
    validSale(decoded, status) &&
    validPaymentDeadline(decoded, status) &&
    validCodReturns(decoded, status)
  );
}

/** 流拍状态不公开成交事实；除“未知”外的成交状态必有买家与成交价，两者成对。 */
function validSale(lot: JsonObject, status: string): boolean {
  const buyerPresent = Object.hasOwn(lot, "buyer_name");
  const pricePresent = Object.hasOwn(lot, "final_price");
  if (!buyerPresent && !pricePresent) {
    return !soldStatus(status);
  }
  if (buyerPresent && pricePresent) {
    return !unsoldStatus(status) && accountNonempty(lot.buyer_name) && positive(lot.final_price);
  }
  return false;
}

/** 付款期限只随三种付款等待状态出现；逾期与重寄后等待付款必有期限。 */
function validPaymentDeadline(lot: JsonObject, status: string): boolean {
  if (Object.hasOwn(lot, "payment_deadline_at")) {
    const deadline = string(lot.payment_deadline_at);
    return (
      (status === "awaiting_payment" ||
        status === "payment_overdue" ||
        status === "cod_resent_awaiting_payment") &&
      deadline !== null &&
      utcSecondTimestamp(deadline)
    );
  }
  return status !== "payment_overdue" && status !== "cod_resent_awaiting_payment";
}

/**
 * 服务端只输出 `YYYY-MM-DDTHH:MM:SSZ`（UTC、秒精度）：逐字节核对形状，再校验日历日期与时分秒
 * 范围。不经浏览器 `Date.parse` 或 RFC 3339 的宽松写法（小数秒、时区偏移），宿主与 wasm 结果一致。
 */
/** 仅供测试使用：核对付款期限的严格 UTC 秒格式。 */
export function utcSecondTimestamp(value: string): boolean {
  const byteLength = utf8ByteLength(value);
  if (byteLength !== 20) {
    return false;
  }
  for (let index = 0; index < byteLength; index += 1) {
    const character = value.charAt(index);
    if (index === 4 || index === 7) {
      if (character !== "-") {
        return false;
      }
    } else if (index === 10) {
      if (character !== "T") {
        return false;
      }
    } else if (index === 13 || index === 16) {
      if (character !== ":") {
        return false;
      }
    } else if (index === 19) {
      if (character !== "Z") {
        return false;
      }
    } else if (compareText(character, "0") < 0 || compareText(character, "9") > 0) {
      return false;
    }
  }
  // 形状已保证这些位置都是 ASCII 数字。
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  return (
    validCivilDate(year, month, day) &&
    Number(value.slice(11, 13)) < 24 &&
    Number(value.slice(14, 16)) < 60 &&
    Number(value.slice(17, 19)) < 60
  );
}

function validCivilDate(year: number, month: number, day: number): boolean {
  if (month < 1 || month > 12 || day < 1) {
    return false;
  }
  return day <= daysInMonth(year, month);
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) {
    return isLeapYear(year) ? 29 : 28;
  }
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}

function isLeapYear(year: number): boolean {
  return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
}

/**
 * 退回次数与冻结上限成对出现：1 ≤ 次数 ≤ 上限 ≤ 10；重寄与达上限状态必有，流拍状态没有。
 * 待寄出 COD 与等待确认收款只属于未发生退回的首次付款：发生过退回后，付款阶段在服务端只会是
 * 退件、重寄、逾期或处理中，带退回次数的这两种状态是矛盾组合，整页失败关闭。
 */
function validCodReturns(lot: JsonObject, status: string): boolean {
  const countPresent = Object.hasOwn(lot, "cod_return_count");
  const limitPresent = Object.hasOwn(lot, "cod_return_limit");
  if (!countPresent && !limitPresent) {
    return !(
      status === "cod_resend_pending" ||
      status === "cod_resent_awaiting_payment" ||
      status === "cod_return_limit_reached"
    );
  }
  if (countPresent && limitPresent) {
    const count = int(lot.cod_return_count);
    const limit = int(lot.cod_return_limit);
    if (count === null || limit === null) {
      return false;
    }
    return (
      !unsoldStatus(status) &&
      status !== "payment_mail_pending" &&
      status !== "awaiting_payment" &&
      count >= 1 &&
      count <= limit &&
      limit >= 1 &&
      limit <= 10 &&
      (status !== "cod_return_limit_reached" || count === limit)
    );
  }
  return false;
}

function unsoldStatus(status: string): boolean {
  return status.startsWith("unsold_");
}

function soldStatus(status: string): boolean {
  return !unsoldStatus(status) && status !== "unknown";
}
