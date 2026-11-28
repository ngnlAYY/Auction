// lot 目录摘要投影的回归断言：完整与精简装备投影得到相同目录行。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import type { TransportResponse } from "@auction/web-shared/net/transport";
import { JSON_MAX_SAFE_INTEGER, type JsonObject, parseJson } from "@auction/web-shared/net/wire";
import { err, ok } from "@auction/web-shared/result";
import { object } from "..";

const { catalogLevelFilterDefault, lotEnvelopeTakeRows } = await import("../../model");
const { decodeLotDetail, decodeLotList } = await import(".");

const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
const SECOND_LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2d";

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

function categoryCounts(values: Readonly<Record<string, number>>): JsonObject {
  return {
    all: 0,
    one: 0,
    two: 0,
    sta: 0,
    shd: 0,
    clo: 0,
    lig: 0,
    hea: 0,
    ...values,
  };
}

function listEnvelope(
  equipment: JsonObject,
  total = 1,
  counts = categoryCounts({ all: 1, clo: 1 }),
): JsonObject {
  return listEnvelopeWithRows([equipment], total, counts);
}

function listEnvelopeWithRows(
  equipments: readonly JsonObject[],
  total: number,
  counts: JsonObject,
): JsonObject {
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
      lots: equipments.map((equipment, index) => ({
        id: index === 0 ? LOT_ID : SECOND_LOT_ID,
        auction_id: AUCTION_ID,
        lot_number: index + 1,
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
      })),
      total,
      page: 1,
      page_size: 20,
      category_counts: counts,
    },
  };
}

function previewListEnvelope(
  equipmentCategory: string | null,
  total: number,
  counts: JsonObject,
): JsonObject {
  return {
    code: "ok",
    message: "拍卖批次已加载",
    data: {
      auction_id: AUCTION_ID,
      auction_status: "preview",
      projection_kind: "preview",
      presentation_mode: "preview",
      bidding_open: false,
      opens_at: "2025-11-15T08:00:00Z",
      preview_revision: 0,
      lots: [
        {
          id: LOT_ID,
          auction_id: AUCTION_ID,
          code: "Clo01",
          status: "pending",
          seller_name: "seller",
          start_price: 1000,
          equipment_name: "Legendary Cotton Robe of Heimdall",
          equipment_group: "rare",
          ...(equipmentCategory === null ? {} : { equipment_category: equipmentCategory }),
          equipment_level: 450,
          primary_attribute_initials: "IW",
          avg_roll: 70.0,
          game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
        },
      ],
      total,
      page: 1,
      page_size: 20,
      category_counts: counts,
    },
  };
}

/// `fields=summary` 的 live 行：只缺属性明细，首字母与 avg_roll 仍由服务端给出。
function summaryEquipment(category = "Cloth"): JsonObject {
  return {
    name: "Legendary Cotton Robe of Heimdall",
    category,
    equipment_group: "rare",
    level: 450,
    primary_attribute_initials: "IW",
    avg_roll: 70.0,
    game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
  };
}

function decodeCatalog(payload: JsonObject, category: string | null = null) {
  const decoded = decodeLotList(
    responseWith(payload),
    AUCTION_ID,
    category,
    null,
    catalogLevelFilterDefault(),
  );
  return decoded;
}

function catalogRows(payload: JsonObject) {
  const decoded = decodeCatalog(payload);
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

test("multi_category_requests_match_rows_and_selected_count_total：多分类按集合核对行与总数", () => {
  const counts = categoryCounts({ all: 4, clo: 1, lig: 2, sta: 1 });
  const selectedRows = [summaryEquipment(), summaryEquipment("Light")];
  // 两个所选分类的计数和是 3；误用 all=4 的旧实现会把这组响应判错。
  expect(decodeCatalog(listEnvelopeWithRows(selectedRows, 3, counts), "clo,lig").ok).toBe(true);
  expect(decodeCatalog(listEnvelopeWithRows(selectedRows, 4, counts), "clo,lig").ok).toBe(false);
  // 总数说还有一行但只给了两行：残缺页必须失败关闭，不能当作已装配的末页。
  expect(decodeCatalog(listEnvelopeWithRows(selectedRows.slice(1), 3, counts), "clo,lig").ok).toBe(
    false,
  );

  // 未选分类时仍以 all 计数核对 total，并接受集合内的任意行。
  const allRows = [...selectedRows, summaryEquipment("Heavy")];
  expect(decodeCatalog(listEnvelopeWithRows(allRows, 4, counts), null).ok).toBe(true);

  const single = listEnvelope(summaryEquipment(), 1, categoryCounts({ all: 1, clo: 1 }));
  expect(decodeCatalog(single, "clo").ok).toBe(true);

  // 第二种行属于 lig，不在请求的 clo、sta 集合内。
  const crossedRows = [summaryEquipment(), summaryEquipment("Light")];
  expect(decodeCatalog(listEnvelopeWithRows(crossedRows, 2, counts), "clo,sta").ok).toBe(false);
  for (const category of ["clo,clo", "all", "unknown"]) {
    expect(decodeCatalog(single, category).ok).toBe(false);
  }
});

test("selected_category_count_overflow_fails_closed：所选分类计数求和超出安全整数时失败关闭", () => {
  const counts = categoryCounts({
    all: JSON_MAX_SAFE_INTEGER,
    clo: JSON_MAX_SAFE_INTEGER - 1,
    lig: 2,
  });
  expect(
    decodeCatalog(listEnvelope(summaryEquipment("Cloth"), JSON_MAX_SAFE_INTEGER, counts), "clo,lig")
      .ok,
  ).toBe(false);
});

test("preview_category_membership_is_checked_against_equipment_category：预览行按装备分类核对集合", () => {
  const counts = categoryCounts({ all: 1, clo: 1 });
  expect(decodeCatalog(previewListEnvelope("Cloth", 1, counts), "clo").ok).toBe(true);
  expect(decodeCatalog(previewListEnvelope(null, 1, counts), null).ok).toBe(true);
  // 缺少分类的旧预览行不能证明属于所选集合。
  expect(decodeCatalog(previewListEnvelope(null, 1, counts), "clo").ok).toBe(false);
});

function fixturePayload(name: string): JsonObject {
  const path = new URL(
    `../../../../../../crates/auction-contracts/tests/fixtures/public/${name}.json`,
    import.meta.url,
  );
  const parsed = parseJson(readFileSync(path, "utf8"));
  const payload = parsed.ok ? object(parsed.value) : null;
  if (payload === null) throw new Error("lot 契约样本必须是 JSON 对象");
  return payload;
}

test("lot 目录和详情接受服务端的 live、预览及 Final 投影", () => {
  for (const [name, status] of [
    ["lots", "live"],
    ["lots-summary", "live"],
    ["lots-preview", "preview"],
    ["lots-final", "final"],
  ] as const) {
    const page = decodeCatalog(fixturePayload(name));
    expect(page.ok && page.value.auction_status).toBe(status);
  }
  for (const name of ["lot", "lot-unleveled", "lot-preview"]) {
    const response = responseWith(fixturePayload(name));
    expect(decodeLotDetail(response, AUCTION_ID, LOT_ID).ok).toBe(true);
    expect(decodeLotDetail(response, AUCTION_ID, AUCTION_ID)).toEqual(err("invalid_response"));
  }
});

test("lot 详情拒绝存储属性中的额外键及错误类型", () => {
  for (const [key, value] of [
    ["prefix", false],
    ["level", "450"],
    ["extra", null],
  ] as const) {
    const payload = fixturePayload("lot-unleveled");
    const lot = object(object(payload.data)?.lot);
    const attributes = object(object(lot?.equipment)?.attributes);
    if (attributes === null) throw new Error("缺少存储属性对象");
    attributes[key] = value;
    expect(decodeLotDetail(responseWith(payload), AUCTION_ID, LOT_ID)).toEqual(
      err("invalid_response"),
    );
  }
});
