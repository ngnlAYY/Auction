// 浏览器侧有界实时防重与版本栅栏。
// `RealtimeCache` 只消费已经通过 wire 解码与序列校验的事件页，不接触网络；
// 它决定页面的资源失效范围，并在事实冲突、版本回退或序列缺口时要求全量重读。

import { int, object, string } from "../decoder";
import type { RealtimeEvent, RealtimePage, RealtimePageMerge } from "./protocol";

const SEEN_REALTIME_LIMIT = 256;
const MAX_SAFE_INTEGER = Number.MAX_SAFE_INTEGER;

interface SeenSet {
  order: string[];
  members: Set<string>;
}

function newSeenSet(): SeenSet {
  return { order: [], members: new Set() };
}

function seenContains(seen: SeenSet, value: string): boolean {
  return seen.members.has(value);
}

/**
 * 记录一个新键；超出上限时返回被淘汰的最旧键，供关联缓存同步删除。
 * 重复键不改变原有位置；调用方在重复路径上已直接忽略事件，不依赖重排续期。
 */
function seenRemember(seen: SeenSet, value: string): string | null {
  if (seen.members.has(value)) {
    return null;
  }
  seen.members.add(value);
  seen.order.push(value);
  if (seen.order.length > SEEN_REALTIME_LIMIT) {
    const oldest = seen.order.shift();
    if (oldest !== undefined) {
      seen.members.delete(oldest);
      return oldest;
    }
  }
  return null;
}

interface LastBidFact {
  fact_index: number;
  state_version: number;
}

interface CacheState {
  auctionId: string;
  initialPreviewRevision: number;
  after: number;
  /** refetch 后每个尚未见过的 lot 可用首个连续 stream 事件建立一次事实基线。 */
  baselineAfter: number | null;
  factIdentities: Map<string, string>;
  lastFacts: Map<string, LastBidFact>;
  seen: SeenSet;
  seenFacts: SeenSet;
  versions: Map<string, number>;
}

function newState(auctionId: string, previewRevision: number): CacheState {
  return {
    auctionId,
    initialPreviewRevision: previewRevision,
    after: 0,
    baselineAfter: null,
    factIdentities: new Map(),
    lastFacts: new Map(),
    seen: newSeenSet(),
    seenFacts: newSeenSet(),
    versions: new Map([[`preview:${auctionId}`, previewRevision]]),
  };
}

/**
 * 旧版单拍卖浏览器维护的有界实时防重与版本栅栏。
 * `after` 是 stream cursor；event ID 与 bid fact identity 是两套独立防重键。发生事实冲突、
 * state/fact 回退或 sequence 缺口时清空局部事实缓存，但保留已确认的最大 cursor。
 */
export interface RealtimeCache {
  after(): number;
  mergePage(page: RealtimePage): RealtimePageMerge;
}

/** 从页面当前预览 revision 建立初始实时缓存。 */
export function createRealtimeCache(auctionId: string, previewRevision: number): RealtimeCache {
  return makeCache(auctionId, previewRevision, 0, false);
}

/** owner 因权威快照 revision 变化而重建时，从已确认 cursor 建立新基线，避免从零重放。 */
export function createRealtimeCacheAfter(
  auctionId: string,
  previewRevision: number,
  after: number,
): RealtimeCache {
  return makeCache(auctionId, previewRevision, after, true);
}

function makeCache(
  auctionId: string,
  previewRevision: number,
  initialAfter: number,
  bounded: boolean,
): RealtimeCache {
  const state = newState(auctionId, previewRevision);
  if (bounded) {
    state.after = Math.max(initialAfter, 0);
    state.baselineAfter = state.after > 0 ? state.after : null;
  }
  return {
    after: () => state.after,
    mergePage: (page) => mergePage(state, page),
  };
}

/**
 * 按旧版 `sync()` 顺序合并一页：refetch 页不消费事件，普通页先逐事件合并，再接受
 * `next_after` 的单调前移。返回 false 时只停止本轮 HTTP catch-up，不关闭 WebSocket。
 */
function mergePage(state: CacheState, page: RealtimePage): RealtimePageMerge {
  if (page.refetch_required) {
    resetAfter(state, page.next_after);
    return {
      refresh_required: true,
      bids_changed: false,
      continue_catch_up: false,
      degraded: true,
    };
  }

  let refreshRequired = false;
  let bidsChanged = false;
  for (const event of page.events) {
    const merged = mergeEvent(state, event);
    if (merged.kind === "gap") {
      resetAfter(state, page.next_after);
      return {
        refresh_required: true,
        bids_changed: false,
        continue_catch_up: false,
        degraded: true,
      };
    }
    if (merged.kind === "applied") {
      refreshRequired = refreshRequired || merged.refresh;
    } else if (merged.kind === "bid_applied") {
      bidsChanged = true;
    }
  }
  if (page.next_after > state.after) {
    state.after = page.next_after;
  }
  return {
    refresh_required: refreshRequired,
    bids_changed: bidsChanged,
    continue_catch_up: true,
    degraded: false,
  };
}

type EventMerge =
  | { kind: "ignored" }
  | { kind: "applied"; refresh: boolean }
  | { kind: "bid_applied" }
  | { kind: "gap" };

function mergeEvent(state: CacheState, event: RealtimeEvent): EventMerge {
  if (event.stream_sequence <= state.after || seenContains(state.seen, event.event_id)) {
    return { kind: "ignored" };
  }
  const expected = checkedAddOne(state.after);
  if (expected === null || expected !== event.stream_sequence) {
    return { kind: "gap" };
  }
  if (event.kind === "lot.bid_accepted") {
    return mergeAcceptedBid(state, event);
  }
  const version = eventVersion(event);
  if (version === null) {
    return { kind: "gap" };
  }
  const [key, value] = version;
  const newer = value > (state.versions.get(key) ?? 0);
  state.after = event.stream_sequence;
  seenRemember(state.seen, event.event_id);
  if (newer) {
    state.versions.set(key, value);
  }
  return { kind: "applied", refresh: newer };
}

function mergeAcceptedBid(state: CacheState, event: RealtimeEvent): EventMerge {
  const payload = object(event.payload);
  if (payload === null) {
    return { kind: "gap" };
  }
  const lotId = string(payload.lot_id);
  const bidId = string(payload.bid_id);
  const stateVersion = int(payload.state_version);
  const factIndex = int(payload.fact_index);
  const amount = int(payload.amount);
  const mechanism = string(payload.mechanism);
  const effectiveEnd = string(payload.effective_end);
  if (
    lotId === null ||
    bidId === null ||
    stateVersion === null ||
    factIndex === null ||
    amount === null ||
    mechanism === null ||
    effectiveEnd === null
  ) {
    return { kind: "gap" };
  }
  const identity = `${lotId}:${stateVersion}:${factIndex}:${amount}:${mechanism}:${effectiveEnd}`;
  const storedIdentity = state.factIdentities.get(bidId);
  if (storedIdentity !== undefined) {
    if (storedIdentity !== identity) {
      return { kind: "gap" };
    }
    seenRemember(state.seen, event.event_id);
    state.after = event.stream_sequence;
    return { kind: "applied", refresh: false };
  }

  const previousVersion = state.versions.get(lotId);
  const expectedVersion = previousVersion === undefined ? null : checkedAddOne(previousVersion);
  if (
    (previousVersion !== undefined &&
      (expectedVersion === null || stateVersion !== expectedVersion)) ||
    !orderedFactIndex(state, lotId, stateVersion, factIndex, event.stream_sequence)
  ) {
    return { kind: "gap" };
  }

  // 校验全部通过后才写入状态，Gap 路径不会污染防重集合。
  seenRemember(state.seen, event.event_id);
  const evicted = seenRemember(state.seenFacts, bidId);
  if (evicted !== null) {
    state.factIdentities.delete(evicted);
  }
  state.factIdentities.set(bidId, identity);
  state.lastFacts.set(lotId, { fact_index: factIndex, state_version: stateVersion });
  state.versions.set(lotId, stateVersion);
  state.after = event.stream_sequence;
  return { kind: "bid_applied" };
}

function orderedFactIndex(
  state: CacheState,
  lotId: string,
  stateVersion: number,
  factIndex: number,
  streamSequence: number,
): boolean {
  const previous = state.lastFacts.get(lotId);
  return (
    stateVersion > 0 &&
    factIndex > 0 &&
    (factIndex === 1 ||
      (previous !== undefined &&
        previous.state_version === stateVersion - 1 &&
        previous.fact_index + 1 === factIndex) ||
      (!state.lastFacts.has(lotId) &&
        state.baselineAfter !== null &&
        streamSequence > state.baselineAfter))
  );
}

function resetAfter(state: CacheState, nextAfter: number): void {
  const after = Math.max(state.after, nextAfter);
  state = Object.assign(state, newState(state.auctionId, state.initialPreviewRevision), {
    after,
    baselineAfter: after,
  });
}

function checkedAddOne(value: number): number | null {
  return value < MAX_SAFE_INTEGER ? value + 1 : null;
}

function eventVersion(event: RealtimeEvent): readonly [string, number] | null {
  const payload = object(event.payload);
  if (payload === null) {
    return null;
  }
  const auctionId = string(payload.auction_id);
  const stateVersion = int(payload.state_version);
  const previewRevision = int(payload.preview_revision);
  if (auctionId === null) {
    return null;
  }
  switch (event.kind) {
    case "auction.created":
      return stateVersion === null ? null : [`created:${auctionId}`, stateVersion];
    case "auction.preview_updated":
    case "preview_catalog_changed":
      return previewRevision === null ? null : [`preview:${auctionId}`, previewRevision];
    case "auction.opened":
      return previewRevision === null ? null : [`opened:${auctionId}`, previewRevision];
    case "auction.final_phase_entered":
    case "auction.schedule_updated":
    case "auction.early_settlement_requested":
    case "auction.cancelled":
    case "auction.settling":
    case "auction.settled":
      return stateVersion === null ? null : [auctionId, stateVersion];
    default:
      return null;
  }
}
