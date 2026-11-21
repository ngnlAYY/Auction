// 寄售登记、改价、取消的请求及可重放的安全回执。

import * as v from "valibot";

import { integer, requiredNullable, strictStruct } from "../../../wire";
import {
  ConsignmentUuidSchema,
  ListingFeeBpsSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "./scalar";
import {
  ActiveListingStateSchema,
  type ListingMode,
  ListingModeSchema,
  ListingStateSchema,
} from "./states";

/** 不变量 1、6、7：稳定命令 ID 和整数标价，装备链接仅用于登记请求。 */
export interface RegisterListing {
  command_id: string;
  mode: ListingMode;
  equipment_url: string;
  price: number;
  expected_fee_bps: number;
}

/** 登记请求的线协议校验，链接语义由服务端校验。 */
export const RegisterListingSchema = strictStruct({
  command_id: ConsignmentUuidSchema,
  mode: ListingModeSchema,
  equipment_url: v.string(),
  price: ListingPriceSchema,
  expected_fee_bps: ListingFeeBpsSchema,
});

/** 不变量 1、6：改价原样重试时保持命令 ID、预期版本与标价。 */
export interface RepriceListing {
  command_id: string;
  expected_version: number;
  price: number;
}

/** 改价请求的闭集线协议。 */
export const RepriceListingSchema = strictStruct({
  command_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
  price: ListingPriceSchema,
});

/** 不变量 1、6：取消请求绑定稳定命令 ID 与预期版本。 */
export interface WithdrawListing {
  command_id: string;
  expected_version: number;
}

/** 取消请求的闭集线协议。 */
export const WithdrawListingSchema = strictStruct({
  command_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
});

const LISTING_OPERATION = v.picklist(["register", "reprice", "withdraw", "admin_delist"]);
const SUCCESSFUL_DISPOSITION = v.picklist(["registered", "repriced", "unchanged", "closed"]);

/** 可通过 HTTP 获得回执的操作。 */
export type ListingOperation = v.InferOutput<typeof LISTING_OPERATION>;

const RECEIPT_FIELDS = {
  command_id: ConsignmentUuidSchema,
  operation: LISTING_OPERATION,
  listing_id: requiredNullable(ConsignmentUuidSchema),
  listing_version: requiredNullable(ListingVersionSchema),
  state: requiredNullable(ListingStateSchema),
  price: requiredNullable(ListingPriceSchema),
};

/** 不变量 1、6、7：写回执只接受与原操作相符的成功处置和目标。 */
export const CommandReceiptSchema = v.pipe(
  strictStruct({ ...RECEIPT_FIELDS, disposition: SUCCESSFUL_DISPOSITION }),
  v.check(({ operation, disposition, listing_id, listing_version, state, price }) => {
    if (listing_id === null || listing_version === null) return false;
    if (disposition === "registered") {
      return (
        operation === "register" && listing_version === 1 && state === "resolving" && price !== null
      );
    }
    if (disposition === "closed") {
      return (operation === "withdraw" || operation === "admin_delist") && state === "closed";
    }
    return operation === "reprice" && v.is(ActiveListingStateSchema, state) && price !== null;
  }),
);
/** 登记、改价、取消与下架使用同一成功回执。 */
export type CommandReceipt = v.InferOutput<typeof CommandReceiptSchema>;

/** 私有查命令可以读到原始拒绝，拒绝中的目标字段可显式为空。 */
export const StoredCommandReceiptSchema = v.union([
  CommandReceiptSchema,
  strictStruct({ ...RECEIPT_FIELDS, disposition: v.literal("rejected") }),
]);
/** 查命令账本所记录的首次回执。 */
export type StoredCommandReceipt = v.InferOutput<typeof StoredCommandReceiptSchema>;

// §7.1 未单列结果码，沿用 Rust 已明确可落账的闭集。
const STORED_COMMAND_HTTP_STATUS = {
  ok: 200,
  accepted: 202,
  participant_ineligible: 403,
  listing_not_found: 404,
  already_open: 409,
  equipment_busy: 409,
  history_unproven: 409,
  history_owner_mismatch: 409,
  history_alias_conflict: 409,
  fee_changed: 409,
  stale_version: 409,
  listing_not_active: 409,
  world_unavailable: 409,
  registration_rate_limited: 429,
  listing_limit_reached: 429,
};

/** 已仲裁并落账的安全结果码。 */
export type StoredCommandCode = keyof typeof STORED_COMMAND_HTTP_STATUS;

/** 首次业务状态、结果码和处置必须一致，外层 GET 状态不参与判定。 */
export const StoredCommandResultSchema = v.pipe(
  strictStruct({
    http_status: integer,
    code: v.custom<StoredCommandCode>(
      (input) => typeof input === "string" && Object.hasOwn(STORED_COMMAND_HTTP_STATUS, input),
    ),
    receipt: StoredCommandReceiptSchema,
  }),
  v.check(({ http_status, code, receipt: { disposition } }) => {
    if (http_status !== STORED_COMMAND_HTTP_STATUS[code]) return false;
    if (code === "accepted") return disposition === "registered";
    if (code === "ok") return disposition !== "registered" && disposition !== "rejected";
    return disposition === "rejected";
  }),
);
/** 本人从命令账本查到的首次业务结果。 */
export type StoredCommandResult = v.InferOutput<typeof StoredCommandResultSchema>;

/** 写命令的成功处置及账本中的业务拒绝。 */
export type ListingDisposition = StoredCommandReceipt["disposition"];
