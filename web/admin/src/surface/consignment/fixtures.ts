// Admin 寄售行为测试的纯内存挂单与错误回执。

import type { AdminListing, AdminListingDetail } from "@auction/contracts/dto/admin/consignment";
import type {
  CommandReceipt,
  EquipmentView,
  OwnedListing,
} from "@auction/contracts/dto/public/consignment";

import type { Reply } from "../../api/api-request-fixture";

/** 测试中的规范 UUID，与实际玩家无关。 */
export const LISTING_ID = "10000000-0000-4000-8000-000000000001";
/** 测试中的卖家 UUID。 */
export const SELLER_ID = "10000000-0000-4000-8000-000000000002";
/** 测试中的独立命令 UUID。 */
export const COMMAND_ID = "10000000-0000-4000-8000-000000000003";
/** 测试读取与写入共用的 API 集合路径。 */
export const LISTINGS = "/api/v1/admin/consignment/listings";
/** 当前挂单的详情路径。 */
export const DETAIL = `${LISTINGS}/${LISTING_ID}`;
/** 当前挂单的下架路径。 */
export const DELIST = `${DETAIL}/delist`;
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

/** 缺省为无快照的在途解析，支持传入服务端的其他合法投影。 */
export function adminListing(
  listing: Partial<OwnedListing> = {},
  record: Partial<Omit<AdminListing, "listing">> = {},
): AdminListing {
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
    can_delist: true,
    ...record,
  };
}

/** 只读挂单可包含关停清退或已下架的元数据。 */
export function adminDetail(
  record: AdminListing,
  other: Partial<AdminListingDetail> = {},
): AdminListingDetail {
  return { record, conflicts: [], conflicts_truncated: false, ...other };
}

/** 已下架的无快照记录。 */
export function delistedListing(reason: string): AdminListing {
  return adminListing(
    {
      version: 4,
      stored_state: "closed",
      effective_state: "closed",
      effective_reason: "admin_delisted",
      view: { kind: "closed", reason: "admin_delisted", equipment: null },
    },
    { can_delist: false, admin_delist_reason: reason },
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

/** 下架已经提交时的安全回执。 */
export function delistReceipt(commandId: string, version = 4): CommandReceipt {
  return {
    command_id: commandId,
    operation: "admin_delist",
    listing_id: LISTING_ID,
    listing_version: version,
    state: "closed",
    disposition: "closed",
    price: null,
  };
}
