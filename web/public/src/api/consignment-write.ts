// 寄售登记、改价与取消的请求，用原命令字段核对成功回执。

import {
  type CommandReceipt,
  CommandReceiptSchema,
  type RegisterListing,
  RegisterListingSchema,
  type RepriceListing,
  RepriceListingSchema,
  type WithdrawListing,
  WithdrawListingSchema,
} from "@auction/contracts/dto/public/consignment";
import { LISTING_PRICE_MAX } from "@auction/contracts/dto/public/consignment/scalar";
import { decode } from "@auction/contracts/wire";
import { err, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

const LISTINGS = "/api/v1/consignment/listings";

/** 成功回执必须绑定原操作、目标、金额和版本，不变量 1、6。 */
export function consignmentReceiptMatches(
  receipt: CommandReceipt,
  commandId: string,
  input:
    | { readonly operation: "register"; readonly price: number }
    | {
        readonly operation: "reprice";
        readonly listing_id: string;
        readonly expected_version: number;
        readonly price: number;
      }
    | {
        readonly operation: "withdraw";
        readonly listing_id: string;
        readonly expected_version: number;
      },
): boolean {
  if (receipt.command_id !== commandId || receipt.operation !== input.operation) return false;
  if (input.operation === "register") return receipt.price === input.price;
  if (receipt.listing_id !== input.listing_id) return false;
  const version = input.expected_version + (receipt.disposition === "unchanged" ? 0 : 1);
  return (
    receipt.listing_version === version &&
    (input.operation === "withdraw" || receipt.price === input.price)
  );
}

type WriteResult = Result<CommandReceipt, ConsignmentPrivateError>;

function invalidCommand(command: { command_id: string; price?: number }): ConsignmentPrivateError {
  if (!validConsignmentUuid(command.command_id)) return { status: 400, code: "invalid_command_id" };
  if (
    "price" in command &&
    (!Number.isSafeInteger(command.price) ||
      (command.price ?? 0) < 1 ||
      (command.price ?? 0) > LISTING_PRICE_MAX)
  ) {
    return { status: 400, code: "invalid_price" };
  }
  return { status: 400, code: "invalid_listing_request" };
}

/** 不变量 1、6、7：登记携带固定命令 ID，链接只进入这一次的请求正文。 */
export async function registerConsignmentListing(
  userId: string,
  command: RegisterListing,
  signal: AbortSignal,
): Promise<WriteResult> {
  const decoded = decode(RegisterListingSchema, command);
  if (!decoded.ok) return err(invalidCommand(command));
  if (utf8ByteLength(decoded.value.equipment_url) > 2048) {
    return err({ status: 400, code: "invalid_equipment_url" });
  }
  const result = await requestPrivateConsignment(
    userId,
    LISTINGS,
    { method: "POST", body: decoded.value },
    202,
    CommandReceiptSchema,
    signal,
  );
  return result.ok &&
    !consignmentReceiptMatches(result.value, command.command_id, {
      operation: "register",
      price: command.price,
    })
    ? err({ status: 202, code: "invalid_response" })
    : result;
}

/** 不变量 1、6：改价使用原版本与原标价。 */
export async function repriceConsignmentListing(
  userId: string,
  listingId: string,
  command: RepriceListing,
  signal: AbortSignal,
): Promise<WriteResult> {
  if (!validConsignmentUuid(listingId)) return err({ status: 400, code: "invalid_listing_id" });
  const decoded = decode(RepriceListingSchema, command);
  if (!decoded.ok) return err(invalidCommand(command));
  const result = await requestPrivateConsignment(
    userId,
    `${LISTINGS}/${listingId}/price`,
    { method: "PATCH", body: decoded.value },
    200,
    CommandReceiptSchema,
    signal,
  );
  return result.ok &&
    !consignmentReceiptMatches(result.value, command.command_id, {
      operation: "reprice",
      listing_id: listingId,
      ...command,
    })
    ? err({ status: 200, code: "invalid_response" })
    : result;
}

/** 不变量 1、6：取消固定目标、版本与命令 ID。 */
export async function withdrawConsignmentListing(
  userId: string,
  listingId: string,
  command: WithdrawListing,
  signal: AbortSignal,
): Promise<WriteResult> {
  if (!validConsignmentUuid(listingId)) return err({ status: 400, code: "invalid_listing_id" });
  const decoded = decode(WithdrawListingSchema, command);
  if (!decoded.ok) return err(invalidCommand(command));
  const result = await requestPrivateConsignment(
    userId,
    `${LISTINGS}/${listingId}/withdraw`,
    { method: "POST", body: decoded.value },
    200,
    CommandReceiptSchema,
    signal,
  );
  return result.ok &&
    !consignmentReceiptMatches(result.value, command.command_id, {
      operation: "withdraw",
      listing_id: listingId,
      ...command,
    })
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
