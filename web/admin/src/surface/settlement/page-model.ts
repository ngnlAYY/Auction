// Admin 结算页的目录解析、报告核验与命令世代类型。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { err, ok, type Result } from "@auction/web-shared/result";

import type { AdminSettlementReport } from "../../api/settlement";
import type {
  AdminSettlementCatalogEntry,
  AdminSettlementCatalogPage,
} from "../../api/settlement-catalog";
import type { AdminActionFeedback } from "../presentation";
import { replaceSettlementQuery } from "./query";
import type { RecoveryConfirmationDraft } from "./recovery";

/** 一页目录的请求身份与结果；身份用于丢弃切换筛选后到达的旧响应。 */
export interface CatalogLoad {
  readonly worldId: string;
  readonly page: number;
  readonly pageSize: number;
  readonly result: Result<AdminSettlementCatalogPage, string>;
}

/** 按期号补查目录的结果；请求身份只含世界与期号。 */
export interface CatalogLookupLoad {
  readonly worldId: string;
  readonly sequence: number | null;
  readonly result: Result<AdminSettlementCatalogPage | null, string>;
}

/** 场次详情的一次读取结果；`auctionId` 用于核对结果仍属于当前选择。 */
export interface DetailLoad<T> {
  readonly auctionId: string;
  readonly result: Result<T | null, string>;
}

/** 目录对当前地址的解析状态：处理中、命中某一场次或已证实矛盾。 */
export type CatalogSelection =
  | { readonly kind: "resolving" }
  | { readonly kind: "matched"; readonly entry: AdminSettlementCatalogEntry }
  | { readonly kind: "contradictory" };

/** 只返回仍是当前世界与期号的那次补查，切换筛选后旧结果不再参与解析。 */
export function matchingLookup(
  load: CatalogLookupLoad | null,
  world: string,
  sequence: number | null,
): CatalogLookupLoad | null {
  return load !== null && load.worldId === world && load.sequence === sequence ? load : null;
}

/** 按地址与目录确定当前场次；有 UUID 时只认该记录，不取同页第一项代替。 */
export function resolveCatalogSelection(
  page: AdminSettlementCatalogPage | null,
  lookup: AdminSettlementCatalogPage | null,
  auctionId: string,
  sequence: number | null,
): CatalogSelection {
  if (page === null) {
    return { kind: "resolving" };
  }
  if (sequence === null) {
    if (auctionId !== "") {
      return { kind: "resolving" };
    }
    const first = page.auctions[0];
    return first === undefined ? { kind: "resolving" } : { kind: "matched", entry: first };
  }

  // 有 UUID 时只能解析到该记录。不同世界可以复用同一个期号，不能取查询的第一项代替。
  const entry = page.auctions.find((candidate) =>
    auctionId === "" ? candidate.sequence === sequence : candidate.auctionId === auctionId,
  );
  if (entry !== undefined) {
    return entry.sequence === sequence ? { kind: "matched", entry } : { kind: "contradictory" };
  }
  if (lookup === null) {
    return { kind: "resolving" };
  }
  const found = lookup.auctions.find(
    (candidate) =>
      candidate.sequence === sequence && (auctionId === "" || candidate.auctionId === auctionId),
  );
  return found === undefined ? { kind: "contradictory" } : { kind: "matched", entry: found };
}

/** 关闭旧场次的操作上下文：两个报告分页、确认草稿、原因与反馈，并推进选择世代。 */
export function resetDetailState(
  lotPage: Cell<number>,
  recoveryPage: Cell<number>,
  confirmation: Cell<RecoveryConfirmationDraft | null>,
  reason: Cell<string>,
  actionFeedback: Cell<AdminActionFeedback | null>,
  selectionGeneration: Cell<number>,
): void {
  lotPage.set(1);
  recoveryPage.set(1);
  confirmation.set(null);
  reason.set("");
  actionFeedback.set(null);
  selectionGeneration.update((current) =>
    current >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : current + 1,
  );
}

const MAX_SAFE_GENERATION = BigInt(Number.MAX_SAFE_INTEGER);

function generationNumber(value: bigint): number {
  return value > MAX_SAFE_GENERATION ? Number.MAX_SAFE_INTEGER : Number(value);
}

/** 未决宿主以 `bigint` 推进世代；恢复报告沿用原页面的 `number` 世代形状。 */
export function generationNumberCell(reload: Cell<bigint>): Cell<number> {
  const cell: Cell<number> = Object.assign(() => generationNumber(reload()), {
    peek: () => generationNumber(reload.peek()),
    set: (value: number) => {
      const bounded = Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
      reload.set(BigInt(bounded));
    },
    update: (change: (current: number) => number) => {
      reload.update((current) => BigInt(change(generationNumber(current))));
    },
  });
  return cell;
}

/** 核对详情报告与目录命中的是同一场次；不一致按源码返回 `invalid_response`。 */
export function reportForSelection(
  selection: CatalogSelection,
  report: AdminSettlementReport | null,
): Result<AdminSettlementReport | null, string> {
  if (selection.kind !== "matched" || report === null) {
    return ok(null);
  }
  const entry = selection.entry;
  if (
    report.auction.id !== entry.auctionId ||
    report.auction.world_id !== entry.worldId ||
    report.auction.sequence !== entry.sequence ||
    report.auction.status !== "settled"
  ) {
    return err("invalid_response");
  }
  return ok(report);
}

/** 目录归并需要的响应身份、地址版本与全部选择状态。 */
export interface CatalogReconcileState {
  readonly load: CatalogLoad;
  /** 归并计算时已恢复到本地筛选的位置版本。 */
  readonly appliedLocationRevision: number;
  /** 当前位置版本，用 peek 读取，避免 apply 阶段建立订阅。 */
  readonly locationRevision: Cell<number>;
  readonly currentWorld: string;
  readonly currentPage: number;
  readonly currentPageSize: number;
  readonly currentId: string;
  readonly desiredSequence: number | null;
  readonly lookup: CatalogLookupLoad | null;
  readonly worldFilter: Cell<string>;
  readonly catalogPage: Cell<number>;
  readonly catalogPageSize: Cell<number>;
  readonly selectedAuctionId: Cell<string>;
  readonly requestedSequence: Cell<number | null>;
  readonly lotPage: Cell<number>;
  readonly recoveryPage: Cell<number>;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
  readonly selectionGeneration: Cell<number>;
}

function reconciliationIsCurrent(state: CatalogReconcileState): boolean {
  // 同页导航及前进后退可能与响应在同一轮到达；先确认新地址已恢复到本地筛选。
  // 历史写入失败时位置版本不变，本地筛选仍可接收匹配的目录。
  if (state.locationRevision.peek() !== state.appliedLocationRevision) {
    return false;
  }
  const world = state.worldFilter.peek();
  const selected = state.selectedAuctionId.peek();
  const sequence = state.requestedSequence.peek();
  const page = state.catalogPage.peek();
  const pageSize = state.catalogPageSize.peek();
  // apply 可能晚于新的位置恢复 effect：计算时的筛选快照与当前状态必须仍一致。
  if (
    world !== state.currentWorld ||
    selected !== state.currentId ||
    sequence !== state.desiredSequence ||
    page !== state.currentPage ||
    pageSize !== state.currentPageSize
  ) {
    return false;
  }
  // 旧目录响应只能归并到发起它的同一筛选页。
  return (
    state.load.worldId === world && state.load.page === page && state.load.pageSize === pageSize
  );
}

/** 目录总数收缩时，空尾页由目录归并钳制到有数据的页。 */
export function isEmptyCatalogTail(page: AdminSettlementCatalogPage): boolean {
  return page.auctions.length === 0 && page.total > 0 && page.page > 1;
}

/** 把已加载目录与当前筛选对齐：空尾页钳制、深链接解析并规范化地址。 */
export function reconcileSettlementCatalog(state: CatalogReconcileState): void {
  const load = state.load;
  if (!load.result.ok || !reconciliationIsCurrent(state)) {
    return;
  }
  const page = load.result.value;
  if (isEmptyCatalogTail(page)) {
    const lastPage = Math.max(Math.trunc((page.total - 1) / page.pageSize) + 1, 1);
    const clamped = Math.min(lastPage, Math.trunc(100_000 / page.pageSize) + 1);
    state.catalogPage.set(clamped);
    // 钳制空页只改页码；仍须解析原深链接的 UUID 与期号。
    replaceSettlementQuery({
      worldId: state.currentWorld,
      auctionId: state.currentId,
      sequence: state.desiredSequence,
      page: clamped,
    });
    return;
  }

  const lookupLoad = matchingLookup(state.lookup, state.currentWorld, state.desiredSequence);
  const lookup = lookupLoad?.result.ok ? lookupLoad.result.value : null;
  const resolution = resolveCatalogSelection(page, lookup, state.currentId, state.desiredSequence);
  if (resolution.kind !== "matched") {
    // 深链接未解析或已证实矛盾均保留地址，不能静默改为最新一期。
    return;
  }
  const selected = resolution.entry;
  if (selected.auctionId !== state.currentId) {
    state.selectedAuctionId.set(selected.auctionId);
    resetDetailState(
      state.lotPage,
      state.recoveryPage,
      state.confirmation,
      state.reason,
      state.actionFeedback,
      state.selectionGeneration,
    );
  }
  if (state.desiredSequence !== selected.sequence) {
    state.requestedSequence.set(selected.sequence);
  }
  replaceSettlementQuery({
    worldId: state.currentWorld,
    auctionId: selected.auctionId,
    sequence: selected.sequence,
    page: state.currentPage,
  });
}
