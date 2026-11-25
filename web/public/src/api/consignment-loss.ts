// 本人读取寄售损失案件回执：只读，没有任何付款或发行能力。
//
// 只读接口：`GET /api/v2/me/consignment/positions/{position_id}/loss-case`（R6 §6.2），
// 私有结果 no-store，非本人统一 404。

import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { strictStruct } from "@auction/contracts/wire";
import { err, type Result } from "@auction/web-shared/result";
import * as v from "valibot";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

// 与服务端 `SellerLossCaseView`（recovery/loss/reads.rs）与 D9 §5.3 一致的五字段；严格闭集，
// 服务端多给任何字段（尤其金额）即整体失败。共享夹具 consignment-loss-case-seller.json。
/** 卖家可见的损失案件：编号、状态与最近一份回执。 */
export const LossCaseSchema = strictStruct({
  position_id: ConsignmentUuidSchema,
  loss_case_id: ConsignmentUuidSchema,
  state: v.picklist(["opened", "reviewing", "closed"]),
  recorded_at: ConsignmentTimestampSchema,
  receipt_id: ConsignmentUuidSchema,
});
/** 损失案件回执。 */
export type LossCase = v.InferOutput<typeof LossCaseSchema>;

/** 读取本人某代管件的损失案件；回显的 position 必须是所请求的目标。 */
export async function fetchLossCase(
  userId: string,
  positionId: string,
  signal: AbortSignal,
): Promise<Result<LossCase, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(positionId)) return err({ status: 400, code: "invalid_request" });
  const result = await requestPrivateConsignment(
    userId,
    `/api/v2/me/consignment/positions/${positionId}/loss-case`,
    { method: "GET" },
    200,
    LossCaseSchema,
    signal,
    [],
  );
  return result.ok && result.value.position_id !== positionId
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
