// 寄售命令回执的读取和请求目标核对；本人列表与详情读取见 consignment-owned-v2。

import {
  type StoredCommandResult,
  StoredCommandResultSchema,
} from "@auction/contracts/dto/public/consignment";
import { err, type Result } from "@auction/web-shared/result";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

const OWNED = "/api/v1/me/consignment";
const STORED_RECEIPT_INTEGERS = [
  ["data", "http_status"],
  ["data", "receipt", "listing_version"],
  ["data", "receipt", "price"],
] as const;

/** 账本查询的 GET 200 只包裹原写入结果；只有匹配的命令可交给恢复界面。 */
export async function fetchConsignmentCommand(
  userId: string,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<StoredCommandResult, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(commandId)) return err({ status: 400, code: "invalid_command_id" });
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED}/commands/${commandId}`,
    { method: "GET" },
    200,
    StoredCommandResultSchema,
    signal,
    STORED_RECEIPT_INTEGERS,
  );
  return result.ok &&
    (result.value.receipt.command_id !== commandId ||
      result.value.receipt.operation === "admin_delist")
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
