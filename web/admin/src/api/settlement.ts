// Admin 结算报告 HTTP 客户端。
// 结算报告只读取后端的安全投影；浏览器不接触数据库结算记录，也不在客户端重新计算
// 买卖双方金额。金额和资金池摘要必须以同一份服务端快照为准。

import {
  type AdminSettlementAuctionResponse,
  type AdminSettlementLotResponse,
  type AdminSettlementReportResponse,
  type AdminSettlementSummaryResponse,
  type AdminStrictSettlementReportResponse,
  AdminStrictSettlementReportResponseSchema,
} from "@auction/contracts/dto/admin/settlement";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { type AdminAuction, decodeAuction } from "./auctions";
import { decodeAdminReceipt, invalidResponseError, responseError } from "./failure";
import { type AdminSessionHost, adminSessionHostLive } from "./session";
import { sendGet } from "./transport";
import { validAdminTime, validCount, validOptionalText, validTrimmedText } from "./validation";

/** 对外保留源码中的短名；DTO 仍以 contracts 的类型为准。 */
export type AdminSettlementLot = AdminSettlementLotResponse;
export type AdminSettlementReport = AdminSettlementReportResponse;
export type AdminSettlementSummary = AdminSettlementSummaryResponse;

/**
 * 旧结算 decoder 只对内嵌 auction（由 contracts 闭集 `AdminAuctionResponse` 承担）使用字段闭集；
 * report、summary 与 lot 允许服务端追加字段，contracts 的 `AdminSettlementReportResponse`/
 * `AdminSettlementSummaryResponse`/`AdminSettlementLotResponse`/
 * `AdminSettlementAdjustmentResponse` 不能加 `deny_unknown_fields`，因此保留开放式镜像，
 * 继续要求旧字段显式存在并逐项校验。同步义务：上述 DTO 字段集变化时必须同步本镜像；
 * 下方构造 DTO 的穷举字段列表会在编译期捕捉漂移。
 */
type StrictSettlementReport = AdminStrictSettlementReportResponse;
type StrictSettlementSummary = NonNullable<StrictSettlementReport["settlement"]>;
type StrictSettlementLot = StrictSettlementReport["lots"][number];

/** 结算读取只用于展示；报告、汇总、拍品与调整允许服务端追加字段，因此只对已声明的整数
 * 字段按 serde 的整数 visitor 拒绝小数词法，未知扩展字段仍按原兼容性接受。决定恢复入口与
 * 场次核验的重复键在任意层拒绝。 */
const ADMIN_SETTLEMENT_REPORT_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "auction", "sequence"],
    ["data", "auction", "preview_revision"],
    ["data", "auction", "state_version"],
    ["data", "auction", "lot_count"],
    ["data", "settlement", "opening_balance"],
    ["data", "settlement", "tax_income"],
    ["data", "settlement", "discount_demand"],
    ["data", "settlement", "compensation_out"],
    ["data", "settlement", "closing_balance"],
    ["data", "settlement", "movement_count"],
    ["data", "lots", null, "lot_number"],
    ["data", "lots", null, "final_price"],
    ["data", "lots", null, "listed_price"],
    ["data", "lots", null, "buyer_amount"],
    ["data", "lots", null, "seller_amount"],
    ["data", "lots", null, "tax_amount"],
    ["data", "lots", null, "compensation_amount"],
    ["data", "lots", null, "hold_ppm"],
    ["data", "lots", null, "adjustment_ppm"],
    // 赢家领先纳秒的 i64 上界与 `2^53` 以上精度按 AC4 裁定暂缓：JSON.parse 后已无法与
    // 合法值区分，等待主线统一接入展示时长原语；本轮只按整数词法校验。
    ["data", "lots", null, "leader_held_nanos"],
  ],
};

/** 读取指定拍卖的结算报告。 */
export async function fetchAdminSettlement(
  host: AdminSessionHost,
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<AdminSettlementReport, string>> {
  if (!validUuid(auctionId)) {
    return err(invalidResponseError("settlement_auction_id"));
  }
  const url = `/api/v1/admin/auctions/${auctionId}/settlement-report`;
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  // 传输层最后一次存活检查与本次恢复之间仍可能取消或注销；不再消费缓冲的成功响应。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminStrictSettlementReportResponseSchema),
    ADMIN_SETTLEMENT_REPORT_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const report = decodeSettlementReport(payload.value.data);
  if (!report.ok) {
    return err(invalidResponseError(report.error));
  }
  if (report.value.auction.id !== auctionId || report.value.auction.status !== "settled") {
    return err(invalidResponseError("settlement_report_owner"));
  }
  return ok(report.value);
}

function decodeSettlementReport(
  report: StrictSettlementReport,
): Result<AdminSettlementReport, string> {
  const auction = decodeAuction(report.auction);
  if (!auction.ok) {
    return err(auction.error);
  }
  let settlement: AdminSettlementSummary | null = null;
  if (report.settlement !== null) {
    const decoded = decodeSettlement(report.settlement);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    settlement = decoded.value;
  }
  const lots: AdminSettlementLot[] = [];
  for (const lot of report.lots) {
    const decoded = decodeSettlementLot(lot);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    lots.push(decoded.value);
  }
  return ok({ auction: settlementAuction(auction.value), settlement, lots });
}

function decodeSettlement(
  settlement: StrictSettlementSummary,
): Result<AdminSettlementSummary, string> {
  if (
    !validCount(settlement.opening_balance) ||
    !validCount(settlement.tax_income) ||
    !validCount(settlement.discount_demand) ||
    !validCount(settlement.compensation_out) ||
    !validCount(settlement.closing_balance) ||
    !validCount(settlement.movement_count) ||
    !validAdminTime(settlement.settled_at)
  ) {
    return err("settlement_summary");
  }
  return ok({
    opening_balance: settlement.opening_balance,
    tax_income: settlement.tax_income,
    discount_demand: settlement.discount_demand,
    compensation_out: settlement.compensation_out,
    closing_balance: settlement.closing_balance,
    movement_count: settlement.movement_count,
    settled_at: settlement.settled_at,
  });
}

function validEquipmentUrl(value: string): boolean {
  for (const prefix of [
    "https://hentaiverse.org/equip/",
    "https://hentaiverse.org/isekai/equip/",
  ]) {
    if (!value.startsWith(prefix)) {
      continue;
    }
    const externalId = value.slice(prefix.length);
    return externalId.length > 0 && externalId.length <= 32 && asciiDigitsOnly(externalId);
  }
  return false;
}

function asciiDigitsOnly(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

function decodeSettlementLot(lot: StrictSettlementLot): Result<AdminSettlementLot, string> {
  if (
    !validUuid(lot.id) ||
    lot.lot_number <= 0 ||
    !validTrimmedText(lot.status) ||
    !validTrimmedText(lot.equipment_name) ||
    !validTrimmedText(lot.equipment_category) ||
    !validEquipmentUrl(lot.game_equipment_url) ||
    !validOptionalText(lot.seller) ||
    !validOptionalText(lot.winner) ||
    !validCount(lot.final_price) ||
    !validCount(lot.listed_price) ||
    !validCount(lot.buyer_amount) ||
    !validCount(lot.seller_amount) ||
    !validCount(lot.tax_amount) ||
    !validCount(lot.compensation_amount) ||
    lot.hold_ppm < 0 ||
    lot.hold_ppm > 1_000_000 ||
    lot.adjustment_ppm < -1_000_000 ||
    lot.adjustment_ppm > 1_000_000 ||
    !Number.isInteger(lot.leader_held_nanos) ||
    lot.leader_held_nanos < 0 ||
    !validOptionalText(lot.adjustment.policy)
  ) {
    return err("settlement_lot");
  }
  return ok({
    id: lot.id,
    lot_number: lot.lot_number,
    status: lot.status,
    equipment_name: lot.equipment_name,
    equipment_category: lot.equipment_category,
    game_equipment_url: lot.game_equipment_url,
    seller: lot.seller,
    winner: lot.winner,
    final_price: lot.final_price,
    listed_price: lot.listed_price,
    buyer_amount: lot.buyer_amount,
    seller_amount: lot.seller_amount,
    tax_amount: lot.tax_amount,
    compensation_amount: lot.compensation_amount,
    hold_ppm: lot.hold_ppm,
    adjustment_ppm: lot.adjustment_ppm,
    leader_held_nanos: lot.leader_held_nanos,
    adjustment: { policy: lot.adjustment.policy },
  });
}

function settlementAuction(auction: AdminAuction): AdminSettlementAuctionResponse {
  return {
    id: auction.id,
    world_id: auction.world_id,
    sequence: auction.sequence,
    status: auction.status,
    preview_scheduled_at: auction.preview_scheduled_at,
    scheduled_open_at: auction.scheduled_open_at,
    preview_opened_at: auction.preview_opened_at,
    preview_manual_open: auction.preview_manual_open,
    preview_revision: auction.preview_revision,
    opened_at: auction.opened_at,
    final_phase_at: auction.final_phase_at,
    settled_at: auction.settled_at,
    cancelled_at: auction.cancelled_at,
    cancelled_reason: auction.cancelled_reason,
    cancelled_by: auction.cancelled_by,
    state_version: auction.state_version,
    created_at: auction.created_at,
    updated_at: auction.updated_at,
  };
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
