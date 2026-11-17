// lot 目录摘要投影的回归断言：完整与精简装备投影得到相同目录行。

import { expect, test } from "bun:test";

import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonObject } from "@auction/web-shared/net/wire";
import { ok } from "@auction/web-shared/result";

const { catalogLevelFilterDefault, lotEnvelopeTakeRows } = await import("../../model");
const { decodeLotList } = await import(".");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";

function responseWith(payload: JsonObject): TransportResponse {
  return {
    status: 200,
    statusText: "OK",
    headers: new Headers(),
    body: null,
    text: () => "",
    json: () => ok(payload),
  };
}

function listEnvelope(equipment: JsonObject): JsonObject {
  return {
    code: "ok",
    message: "拍卖批次已加载",
    data: {
      auction_id: AUCTION_ID,
      auction_status: "live",
      projection_kind: "live",
      presentation_mode: "live",
      bidding_open: true,
      opens_at: "2025-11-15T08:00:00Z",
      preview_revision: 0,
      lots: [
        {
          id: LOT_ID,
          auction_id: AUCTION_ID,
          lot_number: 1,
          status: "live",
          start_price: 1000,
          current_price: 1500,
          next_bid: 1600,
          has_leader: true,
          leader_name: "leader",
          seller_name: "seller",
          effective_end_at: "2025-11-16T08:00:00Z",
          state_version: 3,
          equipment,
        },
      ],
      total: 1,
      page: 1,
      page_size: 20,
      category_counts: {
        all: 1,
        one: 0,
        two: 0,
        sta: 0,
        shd: 0,
        clo: 1,
        lig: 0,
        hea: 0,
      },
    },
  };
}

/// `fields=summary` 的 live 行：只缺属性明细，首字母与 avg_roll 仍由服务端给出。
function summaryEquipment(): JsonObject {
  return {
    name: "Legendary Cotton Robe of Heimdall",
    category: "Cloth",
    equipment_group: "rare",
    level: 450,
    primary_attribute_initials: "IW",
    avg_roll: 70.0,
    game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
  };
}

function catalogRows(payload: JsonObject) {
  const decoded = decodeLotList(
    responseWith(payload),
    AUCTION_ID,
    null,
    null,
    catalogLevelFilterDefault(),
  );
  expect(decoded.ok).toBe(true);
  if (!decoded.ok) {
    throw new Error("lot 列表必须通过严格解码");
  }
  return lotEnvelopeTakeRows(decoded.value);
}

test("summary_projection_decodes_to_the_same_catalog_rows：摘要投影解码出相同目录行", () => {
  const full = {
    ...summaryEquipment(),
    info_summary: "Robe Lv450",
    stats: {
      "Primary Attributes / Intelligence": { value: "+12.5", base: 10.0, roll: 80.0 },
      "Primary Attributes / Wisdom": { value: "+9", base: 7.0, roll: 60.0 },
      "Holy EDB": { value: "+20%" },
    },
  };
  const fullRows = catalogRows(listEnvelope(full));
  const summaryRows = catalogRows(listEnvelope(summaryEquipment()));
  expect(fullRows.length).toBe(1);
  // 目录行本来就不取属性明细：两种投影得到完全相同的展示行。
  expect(summaryRows).toEqual(fullRows);
  expect(summaryRows[0]?.primaryAttributeInitials).toBe("IW");
  expect(summaryRows[0]?.avgRoll).toBe(70.0);
});
