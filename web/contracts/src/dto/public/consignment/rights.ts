// 受限权利的 Public HTTP 契约：找回、挑战、核验、当前权利与续办。
//
// 权利人没有登录会话；请求与响应都不携带会话令牌，令牌只在 `hv_right` Cookie 里。
// 真实、虚拟与未匹配目标共用同一份受理响应形状。

import * as v from "valibot";

import { strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "./command";
import { ListingWorldSchema } from "./listing-options";
import { ConsignmentTimestampSchema, ConsignmentUuidSchema, ListingVersionSchema } from "./scalar";

/** 受限会话可请求的动作闭集。 */
export const RightActionSchema = v.picklist([
  "continue_order",
  "claim_equipment",
  "claim_payment",
  "withdraw_held",
]);
/** 受限会话可请求的动作。 */
export type RightAction = v.InferOutput<typeof RightActionSchema>;

/** 权利种类；损失案件只读。 */
export const RightKindSchema = v.picklist([
  "continue_order",
  "claim_equipment",
  "claim_payment",
  "withdraw_held",
  "loss_case",
]);
/** 权利种类。 */
export type RightKind = v.InferOutput<typeof RightKindSchema>;

/** 续办处置。 */
export const RightClaimDispositionSchema = v.picklist([
  "claim_accepted",
  "already_fulfilling",
  "already_completed",
]);
/** 续办处置。 */
export type RightClaimDisposition = v.InferOutput<typeof RightClaimDispositionSchema>;

/** 游戏角色名：非空、无首尾空白、无控制字符、至多 64 个字符。 */
export const GameIdentitySchema = v.pipe(
  v.string(),
  v.nonEmpty(),
  v.check((value) => Array.from(value).length <= 64),
  v.check(
    (value) => !/(?:^\p{White_Space}|\p{White_Space}$|[\p{Control}\p{Surrogate}])/u.test(value),
  ),
);

/** 权利定位符形状：`R1-` 加 43 个 base64url 字符；规范尾位由服务端校验。 */
export const RightLocatorSchema = v.pipe(v.string(), v.regex(/^R1-[A-Za-z0-9_-]{43}$/u));

/** 挑战码输入：只限长度与字符类，对错由服务端同形处理。 */
export const ChallengeCodeInputSchema = v.pipe(
  v.string(),
  v.maxBytes(40),
  v.regex(/^[A-Za-z0-9-]+$/u),
);

/** 按游戏身份找回定位符指引。 */
export const LocateRightRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  game_identity: GameIdentitySchema,
  world: ListingWorldSchema,
});
/** 找回请求。 */
export type LocateRightRequest = v.InferOutput<typeof LocateRightRequestSchema>;

/** 按权利定位符请求挑战码。 */
export const ChallengeRightRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  right_locator: RightLocatorSchema,
});
/** 发码请求。 */
export type ChallengeRightRequest = v.InferOutput<typeof ChallengeRightRequestSchema>;

/** 用挑战码换取单权利会话。 */
export const VerifyRightRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  challenge_id: CommandUuidV4Schema,
  code: ChallengeCodeInputSchema,
});
/** 核验请求。 */
export type VerifyRightRequest = v.InferOutput<typeof VerifyRightRequestSchema>;

/** 以当前会话办理权利。 */
export const ClaimRightRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
  action: RightActionSchema,
});
/** 续办请求。 */
export type ClaimRightRequest = v.InferOutput<typeof ClaimRightRequestSchema>;

/** 找回与挑战的同形 202 受理结果。 */
export const RightChallengeAcceptedSchema = strictStruct({
  challenge_id: CommandUuidV4Schema,
  expires_at: ConsignmentTimestampSchema,
});
/** 找回与挑战的受理结果。 */
export type RightChallengeAccepted = v.InferOutput<typeof RightChallengeAcceptedSchema>;

/** 核验成功的 200 结果；会话令牌只在 Cookie 里。 */
export const RightVerifiedGrantSchema = strictStruct({
  right_id: ConsignmentUuidSchema,
  right_version: ListingVersionSchema,
  expires_at: ConsignmentTimestampSchema,
});
/** 核验成功结果。 */
export type RightVerifiedGrant = v.InferOutput<typeof RightVerifiedGrantSchema>;

/** 权利指向的合同或代管件。 */
export const RightTargetSchema = v.union([
  strictStruct({ kind: v.literal("contract"), contract_id: ConsignmentUuidSchema }),
  strictStruct({ kind: v.literal("position"), position_id: ConsignmentUuidSchema }),
]);

/** 动作只能是本种类的唯一动作；损失案件没有动作。 */
const ACTION_OF_KIND: Record<RightKind, RightAction | undefined> = {
  continue_order: "continue_order",
  claim_equipment: "claim_equipment",
  claim_payment: "claim_payment",
  withdraw_held: "withdraw_held",
  loss_case: undefined,
};

/** 当前会话对应的权利视图；不含定位符、挑战码或会话令牌。 */
export const RightViewSchema = v.pipe(
  strictStruct({
    right_id: ConsignmentUuidSchema,
    right_version: ListingVersionSchema,
    kind: RightKindSchema,
    world: ListingWorldSchema,
    target: RightTargetSchema,
    allowed_actions: v.array(RightActionSchema),
  }),
  v.check(
    (view) =>
      view.allowed_actions.length <= 1 &&
      view.allowed_actions.every((action) => action === ACTION_OF_KIND[view.kind]),
  ),
);
/** 权利视图。 */
export type RightView = v.InferOutput<typeof RightViewSchema>;

/** 续办命令的受理与回执：同一命令 ID 重放得到同一份。 */
export const RightClaimReceiptSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  right_id: ConsignmentUuidSchema,
  disposition: RightClaimDispositionSchema,
});
/** 续办回执。 */
export type RightClaimReceipt = v.InferOutput<typeof RightClaimReceiptSchema>;
