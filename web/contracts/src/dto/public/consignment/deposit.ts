// 本人寄件指引、预约签发请求与 201 回执。

import * as v from "valibot";

import { strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "./command";
import { ListingWorldSchema } from "./listing-options";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ListingVersionSchema,
} from "./scalar";

const CANONICAL_TAIL = new Set("AEIMQUYcgkosw048");

/** `C1-` 加 43 个无填充 base64url 字符；末位只允许无尾随填充位的 16 个字符。 */
export const DepositReservationCodeSchema = v.pipe(
  v.string(),
  v.regex(/^C1-[A-Za-z0-9_-]{43}$/u),
  v.check((value) => CANONICAL_TAIL.has(value.charAt(45))),
);
/** 寄件预约码。 */
export type DepositReservationCode = v.InferOutput<typeof DepositReservationCodeSchema>;

/** 本人寄件指引。`none` 不携带任何秘密；`active` 才携带当前有效预约码。 */
export const DepositInstructionsSchema = v.union([
  strictStruct({ state: v.literal("none") }),
  strictStruct({
    state: v.literal("active"),
    world: ListingWorldSchema,
    bot: consignmentTextSchema(128),
    subject: v.literal("consign"),
    body: DepositReservationCodeSchema,
    reservation_id: ConsignmentUuidSchema,
    expires_at: ConsignmentTimestampSchema,
  }),
]);
/** 本人寄件指引。 */
export type DepositInstructions = v.InferOutput<typeof DepositInstructionsSchema>;

/** 本人签发或换发寄件预约。不变量 1、6：稳定 UUIDv4 与预期版本。 */
export interface IssueDepositReservation {
  command_id: string;
  expected_version: number;
}

/** 预约签发请求的闭集线协议。 */
export const IssueDepositReservationSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
});

/** 预约签发成功的 201 回执。用户需要寄件码时另读本人指引。 */
export const DepositReservationReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("issue_reservation"),
  disposition: v.literal("reservation_issued"),
  command_id: ConsignmentUuidSchema,
  listing_id: ConsignmentUuidSchema,
  listing_version: ListingVersionSchema,
  state: v.literal("awaiting_deposit"),
  reservation_id: ConsignmentUuidSchema,
  expires_at: ConsignmentTimestampSchema,
});
/** 预约签发回执。 */
export type DepositReservationReceipt = v.InferOutput<typeof DepositReservationReceiptSchema>;
