// Admin 拍卖生命周期命令 API 客户端。
// 浏览器为一次动作输入生成并复用 `command_id`，再提交到版本化 REST 端点；状态迁移、权限、
// 幂等和审计仍由服务端应用端口负责。

import {
  ADMIN_AUCTION_LIFECYCLE_RESPONSE_FIELD_POLICY,
  type AdminAuctionLifecycleResponse,
  AdminAuctionLifecycleResponseSchema,
} from "@auction/contracts/dto/admin/auction";
import type { AdminAuctionLifecycleRequest } from "@auction/contracts/dto/admin/commands";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import {
  type AdminAuctionCommandPreparation,
  decodeAuctionCommandResponse,
  sendAuctionEdit,
} from "./auctions";
import { type AdminWriteFailure, writeNotSent, writeOutcomeUnknown } from "./failure";
import type { AdminSessionHost } from "./session";
import { prepareJsonCommandOnce } from "./transport";

export type AdminAuctionLifecycleResult = AdminAuctionLifecycleResponse;

export type AdminAuctionLifecycleAction =
  | "preview"
  | "open"
  | "cancel"
  | "final-phase"
  | "early-settlement";

/** 冻结生命周期命令的版本与原因；动作只决定 URL，不进入正文。 */
export function prepareAdminAuctionLifecycleOnce(
  expectedStateVersion: number,
  reason: string | null,
): AdminAuctionCommandPreparation {
  return prepareJsonCommandOnce((commandId) => {
    const command: AdminAuctionLifecycleRequest = {
      command_id: commandId,
      expected_state_version: expectedStateVersion,
    };
    if (reason !== null) {
      command.reason = reason;
    }
    return command;
  });
}

export async function executeAdminAuctionLifecycle(
  host: AdminSessionHost,
  commandId: string,
  auctionId: string,
  action: AdminAuctionLifecycleAction,
  prepare: AdminAuctionCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminAuctionLifecycleResult, AdminWriteFailure>> {
  if (!validUuid(auctionId)) {
    return err(writeNotSent("invalid_request"));
  }
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const url = `/api/v1/admin/auctions/${auctionId}/${lifecycleActionPath(action)}`;
  const payload = await sendAuctionEdit(
    host,
    "POST",
    url,
    prepared.value,
    apiResponseSchema(AdminAuctionLifecycleResponseSchema),
    ADMIN_AUCTION_LIFECYCLE_RESPONSE_FIELD_POLICY,
    signal,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const auction = decodeAuctionCommandResponse(payload.value);
  if (!auction.ok) {
    return err(writeOutcomeUnknown(auction.error));
  }
  if (auction.value.id !== auctionId) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok({ auction: auction.value });
}

function lifecycleActionPath(action: AdminAuctionLifecycleAction): string {
  switch (action) {
    case "preview":
      return "preview";
    case "open":
      return "open";
    case "cancel":
      return "cancel";
    case "final-phase":
      return "final-phase";
    case "early-settlement":
      return "early-settlement";
    default:
      return unreachable(action);
  }
}
