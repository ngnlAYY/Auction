// Admin 寄售行为测试的纯内存挂单与错误回执。

import type {
  AdminListingDetailV2,
  AdminListingV2,
} from "@auction/contracts/dto/admin/consignment/listing-v2";
import type { EquipmentView } from "@auction/contracts/dto/public/consignment";
import type { OwnedListingV2 } from "@auction/contracts/dto/public/consignment/listing-v2";

import type { Reply } from "../../api/api-request-fixture";

/** 测试中的规范 UUID，与实际玩家无关。 */
export const LISTING_ID = "10000000-0000-4000-8000-000000000001";
/** 测试中的卖家 UUID。 */
export const SELLER_ID = "10000000-0000-4000-8000-000000000002";
/** 测试中的独立命令 UUID。 */
export const COMMAND_ID = "10000000-0000-4000-8000-000000000003";
/** 测试读取与写入共用的 API 集合路径。 */
export const LISTINGS = "/api/v2/admin/consignment/listings";
/** 当前挂单的详情路径。 */
export const DETAIL = `${LISTINGS}/${LISTING_ID}`;
/** 当前挂单的 v2 下架/止售路径。 */
export const DELIST = `/api/v2/admin/consignment/listings/${LISTING_ID}/delist`;
/** 当前挂单的 v2 解除止售路径。 */
export const RELEASE_HOLD = `/api/v2/admin/consignment/listings/${LISTING_ID}/release-hold`;
/** 当前挂单上有效止售的 ID。 */
export const HOLD_ID = "10000000-0000-4000-8000-000000000004";
/** 浏览器中的挂单详情路径。 */
export const DETAIL_PAGE = `/consignment/listings/${LISTING_ID}`;
/** 购买前可交易的脱敏装备白名单。 */
export const SNAPSHOT: EquipmentView = {
  name: "Fixture sword",
  alt_name: "Fixture blade",
  category: "Sword",
  level: { kind: "assigned", value: 123 },
  tradeable: true,
  stat_groups: [{ name: "Fixture group", stats: [{ name: "Fixture stat", value: 1.4, base: 0 }] }],
  source: "hvdata-equip-v1",
  observed_at: "2026-11-18T00:30:00Z",
};

/** 标价 1,001 的寄售报价样本。 */
export const CONSIGNED_QUOTE = {
  mode: "consigned",
  fee_bps: 500,
  estimated_fee: 50,
  estimated_seller_amount: 951,
} as const;

/** 缺省为无快照的在途解析，支持传入服务端的其他合法投影。 */
export function adminListing(
  listing: Partial<OwnedListingV2> = {},
  record: Partial<Omit<AdminListingV2, "listing">> = {},
): AdminListingV2 {
  return {
    listing: {
      listing_id: LISTING_ID,
      version: 3,
      world_id: "main",
      external_equipment_id: "987654321098765432109876543210",
      mode: "direct",
      price: 1001,
      pricing_version: 1,
      fee_bps: 0,
      estimated_fee: 0,
      estimated_seller_amount: 1001,
      created_at: "2026-11-18T00:00:00Z",
      updated_at: "2026-11-18T00:15:00Z",
      expires_at: "2026-11-19T00:00:00Z",
      generation: null,
      position_id: null,
      loss_case_id: null,
      stored_state: "resolving",
      effective_state: "resolving",
      effective_reason: null,
      allowed_actions: [],
      view: {
        kind: "resolving",
        progress: {
          attempts: 2,
          max_attempts: 6,
          next_attempt_at: "2026-11-18T01:00:00Z",
          last_reason: "source_timeout",
          paused: true,
          deadline_at: "2026-11-19T00:00:00Z",
        },
      },
      ...listing,
    },
    seller_name: "Fixture seller",
    seller_user_id: SELLER_ID,
    admin_delist_reason: null,
    hold: null,
    position: null,
    admin_actions: ["delist"],
    ...record,
  };
}

/** 只读挂单可包含关停清退或已下架的元数据。 */
export function adminDetail(
  record: AdminListingV2,
  other: Partial<AdminListingDetailV2> = {},
): AdminListingDetailV2 {
  return { record, conflicts: [], conflicts_truncated: false, ...other };
}

/** 已下架的无快照记录。 */
export function delistedListing(reason: string): AdminListingV2 {
  return adminListing(
    {
      version: 4,
      stored_state: "closed",
      effective_state: "closed",
      effective_reason: "admin_delisted",
      view: { kind: "closed", reason: "admin_delisted", equipment: null },
    },
    { admin_actions: [], admin_delist_reason: reason },
  );
}

/** 构造可用于现有 Admin 假传输的成功应答。 */
export function success(data: unknown): Reply {
  return { body: JSON.stringify({ code: "ok", message: "", data }) };
}

/** 构造完整错误信封。消息的样本内容不得显示。 */
export function failure(status: number, code: string): Reply {
  return {
    status,
    body: JSON.stringify({ code, message: "Fixture server message", data: null, request_id: null }),
  };
}

/** 未代管件下架已经提交时的安全回执。 */
export function delistReceipt(
  commandId: string,
  version = 4,
): {
  schema_version: 2;
  operation: "admin_delist";
  disposition: "closed";
  command_id: string;
  listing_id: string;
  listing_version: number;
  state: "closed";
} {
  return {
    schema_version: 2,
    command_id: commandId,
    operation: "admin_delist",
    disposition: "closed",
    listing_id: LISTING_ID,
    listing_version: version,
    state: "closed",
  };
}

/** 代管件设置运营止售时的安全回执。 */
export function holdSetReceipt(
  commandId: string,
  version = 4,
): {
  schema_version: 2;
  operation: "admin_delist";
  disposition: "hold_set";
  command_id: string;
  listing_id: string;
  listing_version: number;
  state: "held_off_market";
  hold_id: string;
} {
  return {
    schema_version: 2,
    command_id: commandId,
    operation: "admin_delist",
    disposition: "hold_set",
    listing_id: LISTING_ID,
    listing_version: version,
    state: "held_off_market",
    hold_id: HOLD_ID,
  };
}

/** 带有效运营止售的代管件；动作清单由调用方决定。 */
export function heldListing(
  actions: AdminListingV2["admin_actions"] = ["release_hold"],
): AdminListingV2 {
  return adminListing(
    {
      ...CONSIGNED_QUOTE,
      mode: "consigned",
      generation: 1,
      stored_state: "held_off_market",
      effective_state: "held_off_market",
      effective_reason: "operations_hold",
      view: {
        kind: "held_off_market",
        equipment: SNAPSHOT,
        reason: "operations_hold",
        hold_id: null,
      },
    },
    {
      hold: { hold_id: HOLD_ID, set_at: "2026-11-18T00:20:00Z" },
      position: { position_id: LISTING_ID, state: "in_custody", version: 2, generation: 1 },
      admin_actions: actions,
    },
  );
}

/** 解除止售成功时的安全回执；hold_id 可被测试改成不符值。 */
export function holdReleasedReceipt(
  commandId: string,
  version = 4,
  holdId = HOLD_ID,
): {
  schema_version: 2;
  operation: "release_hold";
  disposition: "hold_released";
  command_id: string;
  listing_id: string;
  listing_version: number;
  state: "held_off_market";
  hold_id: string;
} {
  return {
    schema_version: 2,
    command_id: commandId,
    operation: "release_hold",
    disposition: "hold_released",
    listing_id: LISTING_ID,
    listing_version: version,
    state: "held_off_market",
    hold_id: holdId,
  };
}
