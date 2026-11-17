// Public API 查询参数和目录展示模型。

import {
  EQUIPMENT_CATEGORY_CODES,
  type PublicLiveLotResponse,
  type PublicLotDetailItemResponse,
  type PublicLotEnvelopeResponse,
  type PublicPreviewLotResponse,
} from "@auction/contracts/dto/public/catalog/lot";
import { err, ok, type Result } from "@auction/web-shared/result";

/** 公开价格历史的旧版筛选与分页参数。 */
export interface PriceHistoryQuery {
  world: string | null;
  name: string | null;
  from: string | null;
  to: string | null;
  minPrice: string | null;
  maxPrice: string | null;
  buyer: string | null;
  seller: string | null;
  sort: string;
  page: number;
}

/** 价格历史查询的默认值；created 的字段与 Rust `Default` 实现一致。 */
export function priceHistoryQueryDefault(): PriceHistoryQuery {
  return {
    world: null,
    name: null,
    from: null,
    to: null,
    minPrice: null,
    maxPrice: null,
    buyer: null,
    seller: null,
    sort: "closed_desc",
    page: 1,
  };
}

/** 目录等级查询的浏览器投影；保留整数精度，不把无等级等同于零级。 */
export interface CatalogLevelFilter {
  maxLevel: number | null;
  includeUnleveled: boolean;
}

/** 未设置上限且包含无等级装备的默认等级筛选。 */
export function catalogLevelFilterDefault(): CatalogLevelFilter {
  return { maxLevel: null, includeUnleveled: true };
}

/** 草稿与 URL 共用严格解析；留空仅表示不限，非法值不得悄悄扩大查询。迁移限制：Rust 接受完整
 * `i64`，浏览器数字只能安全表示到 `Number.MAX_SAFE_INTEGER`，更大的等级值按非法输入拒绝。 */
export function catalogLevelFilterParse(
  maxLevelText: string,
  includeUnleveledText: string,
): Result<CatalogLevelFilter, null> {
  let maxLevel: number | null;
  if (maxLevelText === "") {
    maxLevel = null;
  } else {
    for (let index = 0; index < maxLevelText.length; index += 1) {
      const code = maxLevelText.charCodeAt(index);
      if (code < 0x30 || code > 0x39) {
        return err(null);
      }
    }
    const parsed = Number(maxLevelText);
    // Rust 接受完整 i64；TS 只能安全表示到 Number.MAX_SAFE_INTEGER，超出后不扩大查询。
    if (!Number.isSafeInteger(parsed)) {
      return err(null);
    }
    maxLevel = parsed;
  }
  let includeUnleveled: boolean;
  if (includeUnleveledText === "true") {
    includeUnleveled = true;
  } else if (includeUnleveledText === "false") {
    includeUnleveled = false;
  } else {
    return err(null);
  }
  return ok({ maxLevel, includeUnleveled });
}

/** 只用于校验响应是否属于本次查询，不在浏览器分页后过滤或推算总数。 */
export function catalogLevelFilterMatchesLevel(
  filter: CatalogLevelFilter,
  level: number | null,
): boolean {
  if (level !== null) {
    return level >= 0 && (filter.maxLevel === null || level <= filter.maxLevel);
  }
  return filter.includeUnleveled;
}

/** 目录表格所需的展示行。 */
export interface CatalogRow {
  lotId: string;
  code: string;
  name: string;
  gameEquipmentUrl: string;
  category: string;
  equipmentGroup: string;
  status: string;
  sellerName: string;
  level: number | null;
  primaryAttributeInitials: string | null;
  avgRoll: number | null;
  startPrice: number;
  currentPrice: number;
  nextBid: number;
  leaderName: string | null;
  remaining: string;
}

/** lot 列表负载的移出接口；源实现为 `LotEnvelope` 上的 trait，浏览器用自由函数完成同一语义。 */
export interface LotEnvelopeRows {
  takeRows(): CatalogRow[];
}

/** 将稳定 lot wire 契约转换为页面表格所需的展示行，并移出列表负载。
 * 只保留分页与拍卖元数据；目录快照只持有一份装备字符串。 */
export function lotEnvelopeTakeRows(envelope: PublicLotEnvelopeResponse): CatalogRow[] {
  const lots = envelope.lots;
  envelope.lots = null;
  if (lots == null) {
    return [];
  }
  // decoder 已把整个数组判定为同一无标签变体；逐项检查同时保持了对空数组和同形数组的语义。
  const rows: CatalogRow[] = [];
  for (const lot of lots) {
    if (isLiveLot(lot)) {
      rows.push(liveRow(lot));
    } else {
      rows.push(previewRow(lot));
    }
  }
  return rows;
}

/** 单个拍品详情响应对应的展示行；与目录列表共用同一套字段映射，抽屉因此不区分数据来源。 */
export function catalogRowFromDetail(lot: PublicLotDetailItemResponse): CatalogRow {
  if ("equipment_name" in lot) {
    return previewRow(lot);
  }
  return liveRow(lot);
}

/** 预览拍品尚无竞价：当前价与下一口价都取起拍价，没有领先者与截止时间。 */
function previewRow(lot: PublicPreviewLotResponse): CatalogRow {
  return {
    lotId: lot.id,
    code: lot.code,
    name: lot.equipment_name,
    gameEquipmentUrl: lot.game_equipment_url,
    // 缺失分类沿用旧版归入 Other，但不把缺失重新编码成 wire 空字符串。
    category: lot.equipment_category ?? "other",
    equipmentGroup: lot.equipment_group,
    status: lot.status,
    sellerName: lot.seller_name,
    level: lot.equipment_level,
    primaryAttributeInitials: lot.primary_attribute_initials,
    avgRoll: lot.avg_roll,
    startPrice: lot.start_price,
    currentPrice: lot.start_price,
    nextBid: lot.start_price,
    leaderName: null,
    remaining: "",
  };
}

function liveRow(lot: PublicLiveLotResponse): CatalogRow {
  return {
    lotId: lot.id,
    code: lotCodeFor(lot.equipment.category, lot.lot_number),
    name: lot.equipment.name,
    gameEquipmentUrl: lot.equipment.game_equipment_url,
    category: lot.equipment.category,
    equipmentGroup: lot.equipment.equipment_group,
    status: lot.status,
    sellerName: lot.seller_name,
    level: lot.equipment.level,
    primaryAttributeInitials: lot.equipment.primary_attribute_initials,
    avgRoll: lot.equipment.avg_roll,
    startPrice: lot.start_price,
    currentPrice: lot.current_price,
    nextBid: lot.next_bid,
    leaderName: lot.leader_name,
    remaining: lot.effective_end_at ?? "",
  };
}

/** 与论坛、邮件一致的 lot 代码：契约表前缀加至少两位补零序号，未知分类显示 `Lot` 前缀。 */
export function lotCodeFor(category: string, lotNumber: number): string {
  const entry = EQUIPMENT_CATEGORY_CODES.find(([, name]) => name === category);
  const prefix = entry === undefined ? "Lot" : entry[2];
  return `${prefix}${String(lotNumber).padStart(2, "0")}`;
}

function isLiveLot(
  lot: PublicPreviewLotResponse | PublicLiveLotResponse,
): lot is PublicLiveLotResponse {
  return "equipment" in lot;
}
