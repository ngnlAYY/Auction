// 本人 v2 寄售命令回执查询：结果未知的写在重放前先查原回执，404 只代表当次未见（D9 §6.3）。

import {
  type OwnedCommandReceiptV2,
  OwnedCommandReceiptV2Schema,
} from "@auction/contracts/dto/public/consignment/receipt-v2";
import { err, type Result } from "@auction/web-shared/result";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

const COMMANDS = "/api/v2/me/consignment/commands";
const RECEIPT_INTEGERS = [
  ["data", "listing_version"],
  ["data", "contract_version"],
  ["data", "price"],
  ["data", "generation"],
  ["data", "http_status"],
] as const;

/** 回执查询 404 `command_not_found` 只表示当次未见，才授权原样重放。 */
export function commandReceiptMissing(error: ConsignmentPrivateError): boolean {
  return error.status === 404 && error.code === "command_not_found";
}

/** 按会话用户读取原命令回执；回执必须属于所查命令，否则按无效响应处理。 */
export async function fetchOwnedCommandReceiptV2(
  userId: string,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<OwnedCommandReceiptV2, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(commandId)) return err({ status: 400, code: "invalid_command_id" });
  const result = await requestPrivateConsignment(
    userId,
    `${COMMANDS}/${commandId}`,
    { method: "GET" },
    200,
    OwnedCommandReceiptV2Schema,
    signal,
    RECEIPT_INTEGERS,
  );
  return result.ok && result.value.command_id !== commandId
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
