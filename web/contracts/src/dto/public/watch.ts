// 当前会话用户的装备标记 HTTP 契约；取消记录保留正版本 tombstone。

import * as v from "valibot";
import { integer, struct } from "../../wire";

/** 单个装备的权威标记状态。 */
export const PublicLotWatchResponseSchema = struct({
  lot_id: v.string(),
  watched: v.boolean(),
  /** 历史兼容占位，恒为 mm；实际渠道由账号警告偏好决定。 */
  channel: v.string(),
  version: integer,
});

export type PublicLotWatchResponse = v.InferOutput<typeof PublicLotWatchResponseSchema>;

/** 指定拍卖内当前用户的完整标记集合。 */
export const PublicLotWatchesResponseSchema = struct({
  user_id: v.string(),
  auction_id: v.string(),
  watches: v.array(PublicLotWatchResponseSchema),
});

export type PublicLotWatchesResponse = v.InferOutput<typeof PublicLotWatchesResponseSchema>;

/** CAS 写入；请求结果未知时必须保留原命令及完整负载重试。 */
export interface PublicSetLotWatchRequest {
  command_id: string;
  watched: boolean;
  /** 仅允许 mm 兼容占位，不修改账号渠道。 */
  channel: string;
  expected_version: number;
}

/** 可重放的原命令结果；重放后客户端重新读取权威集合。 */
export const PublicSetLotWatchResponseSchema = struct({
  user_id: v.string(),
  auction_id: v.string(),
  watch: PublicLotWatchResponseSchema,
});

export type PublicSetLotWatchResponse = v.InferOutput<typeof PublicSetLotWatchResponseSchema>;
