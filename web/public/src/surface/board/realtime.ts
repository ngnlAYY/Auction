// Public 看板的实时目标、连接状态与唯一的实时连接 owner。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { documentIsVisible, sleepWhileVisible } from "@auction/web-shared/platform/visibility";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { unreachable } from "@auction/web-shared/result";
import type { JSX } from "@solidjs/web";
import { createContext, useContext } from "solid-js";

import { estimatedNow } from "../../api/clock";
import { type RealtimePage, subscribeAuctionRealtime } from "../../api/realtime";
import { createRealtimeCacheAfter } from "../../api/realtime/cache";
import {
  REALTIME_SESSION_CLOSE_CODE,
  realtimeErrorDetail,
  realtimeErrorIsPermanent,
} from "../../api/realtime/protocol";
import { requestPublicSessionCheck } from "../../api/session";
import { invalidateAll } from "../../api/swr-cache";
import {
  type LiveConnectionState,
  normalizedWorldQuery,
  type PublicBoardQuery,
  type WorldScope,
  worldScopeFromQuery,
} from "../query";
import { PageVisibilityContext } from "../shell/contexts";
import { PublicBidRefreshContext } from "./refresh";

/** Public 页头的 readiness 有限状态；只决定外层连接徽章，不改写核心实时四态。 */
export type PublicReadinessState = "checking" | "ready" | "degraded";

/** Public 页头在核心实时四态之外展示的完整七态。 */
export type PublicConnectionState =
  | "connecting"
  | "degraded"
  | "live"
  | "manual"
  | "preview"
  | "idle"
  | "unavailable";

/** 旧版 Public 外壳在任一时刻只拥有一个实时目标。 */
export type PublicRealtimeTarget =
  | { readonly kind: "checking" }
  | { readonly kind: "anonymous" }
  | { readonly kind: "session_error" }
  | {
      readonly kind: "no_target";
      readonly reason: "empty" | "invalid_auction" | "directory_error" | "selection_error";
    }
  /** 页面目标的拍卖身份读取失败；不拿 URL 世界或默认 Main 代替权威归属。 */
  | { readonly kind: "identity_error"; readonly targetId: string }
  /** 已确认未进入竞价期；世界属于目标身份，切换世界不会沿用旧连接目标。 */
  | { readonly kind: "inactive"; readonly world: string; readonly preview: boolean }
  /** 当前场次读取失败，不把失败伪装为没有拍卖或健康的实时连接。 */
  | { readonly kind: "current_error"; readonly world: string }
  | {
      readonly kind: "auction";
      readonly auctionId: string;
      readonly world: string;
      readonly previewRevision: number;
    };

/** 页面实时目标必须绑定明确请求，不能跨显式拍卖查询复用。 */
export type PublicRealtimePageKey =
  | { readonly kind: "auction"; readonly id: string }
  | {
      readonly kind: "bidlog";
      readonly id: string | null;
      readonly invalidAuction: boolean;
      readonly scope: WorldScope;
    };

/** 出价记录的目标绑定到本次地址选择的场次与世界范围。 */
export function bidLogRealtimeKey(query: PublicBoardQuery) {
  return {
    kind: "bidlog",
    id: query.auctionId,
    invalidAuction: query.invalidAuction !== null,
    scope: worldScopeFromQuery("multi", normalizedWorldQuery(query.world)),
  } satisfies PublicRealtimePageKey;
}

/** 页面公布已按拍卖 UUID 解析的实时目标；已结算日志用空目标保留外壳的生命周期订阅。 */
export type PublicPageRealtimeTarget = Cell<
  readonly [PublicRealtimePageKey, PublicRealtimeTarget | null] | null
>;

export const PublicPageRealtimeTargetContext = createContext<PublicPageRealtimeTarget>();

/**
 * 拍卖页按显式 `auction_id` 展示的场次摘要。外壳 ticker 承担页面标题，必须与正文是同一场次；
 * 页面离开或请求变化后残留的值由 `tickerAuction` 按拍卖 UUID 拒绝。
 */
export type PublicDisplayedAuction = Cell<CurrentAuctionItemResponse | null>;

export const PublicDisplayedAuctionContext = createContext<PublicDisplayedAuction>();

/**
 * 选出 ticker 展示的场次。
 *
 * 显式请求某一场次时只接纳页面已加载的同一场次：加载中、加载失败或身份不符都返回 `null`，
 * 不借用所选世界当前场次的编号、状态与倒计时去标注另一场的目录。其余页面显示当前场次。
 */
export function tickerAuction(
  requested: PublicRealtimePageKey | null,
  displayed: CurrentAuctionItemResponse | null,
  current: CurrentAuctionItemResponse | null,
): CurrentAuctionItemResponse | null {
  if (requested !== null && requested.kind === "auction") {
    return displayed !== null && displayed.id === requested.id ? displayed : null;
  }
  return current;
}

/** 顶栏倒计时数向哪个时刻；决定标签与到点后的提示文案。 */
export type TickerCountdown =
  /** 竞价期（live/final）数到计划截止。 */
  | { readonly kind: "closing"; readonly endsAt: string }
  /** 预览期数到开拍。 */
  | { readonly kind: "opening"; readonly opensAt: string };

/**
 * 按展示场次的状态选出顶栏倒计时。
 *
 * 只有竞价期与预览期有“还剩多久”的含义；已结算、结算中、取消等状态即使携带历史截止
 * 时刻也不显示，否则过去的截止会被渲染成“落锤中…”。
 */
export function tickerCountdown(auction: CurrentAuctionItemResponse): TickerCountdown | null {
  switch (auction.status) {
    case "live":
    case "final": {
      const endsAt = auction.scheduled_ends_at;
      return endsAt === null || endsAt === undefined ? null : { kind: "closing", endsAt };
    }
    case "preview":
      return { kind: "opening", opensAt: auction.opens_at };
    default:
      return null;
  }
}

/** 把已经加载的当前/历史场次投影成实时目标；只有竞价期建立连接。 */
export function publicRealtimeTargetFromAuction(
  auction: CurrentAuctionItemResponse,
): PublicRealtimeTarget {
  if (auction.status === "live" || auction.status === "final") {
    return {
      kind: "auction",
      auctionId: auction.id,
      world: auction.world,
      previewRevision: auction.preview_revision,
    };
  }
  return { kind: "inactive", world: auction.world, preview: auction.status === "preview" };
}

/** 重连退避起点：1 秒起步，避免旧版固定 3 秒在故障初期空等。 */
const RECONNECT_BASE_MILLIS = 1_000;
/** 重连退避上限：指数翻倍到 30 秒封顶，限制服务端故障期间的重试压力。 */
const RECONNECT_MAX_MILLIS = 30_000;
/** 连续快速失败达到该次数后认定实时通道不可用：转 Manual 态，页头降级为手动刷新。 */
const RECONNECT_MAX_FAILURES = 8;
/** 单次连接存活超过该时长视为健康会话，断开后重置退避计数。 */
const RECONNECT_HEALTHY_MILLIS = 30_000;

/**
 * 指数退避 + 随机抖动：1s 起步 ×2 至 30s 上限，实际等待取 [base/2, base] 区间，
 * 打散多客户端在服务端恢复后的重连同步。
 */
function reconnectDelayMillis(failures: number): number {
  // 1 << 5 = 32 已覆盖 30s 上限，更大的失败计数不会再抬升底数。
  const shift = Math.min(Math.max(failures - 1, 0), 5);
  const base = Math.min(RECONNECT_BASE_MILLIS * (1 << shift), RECONNECT_MAX_MILLIS);
  const half = Math.trunc(base / 2);
  return half + Math.trunc(Math.random() * (base - half));
}

/**
 * 根据共享 session/current 快照建立唯一实时连接；目标或用户变化时取消旧任务并重新握手。
 * 只有竞价期场次目标建立连接，其余目标只写入连接状态：开拍切换与预览目录更新由外壳的
 * 权威轮询发现。
 */
export function PublicRealtimeOwner(props: {
  readonly target: PublicRealtimeTarget;
  readonly userId: string | null;
  readonly refreshGeneration: Cell<number>;
  readonly auctionAfter: Cell<readonly [string, number] | null>;
  readonly realtimeState: Cell<LiveConnectionState>;
}): JSX.Element {
  const pageVisibility = useContext(PageVisibilityContext);
  const bidRefresh = useContext(PublicBidRefreshContext);
  // 身份必须进入资源依赖；单独修改目标对象的引用不会使相等的 target 重启订阅。
  const dependencies = createValueMemo(() => ({
    target: props.target,
    userId: props.userId,
  }));
  createTaskResource(dependencies, async (current, context) => {
    const target = current.target;
    const visibility = pageVisibility.clone();
    const auctionAfter = props.auctionAfter;
    const realtimeState = props.realtimeState;
    const refreshGeneration = props.refreshGeneration;
    if (target.kind === "checking") {
      realtimeState.set("connecting");
      return;
    }
    if (target.kind === "anonymous") {
      realtimeState.set("manual");
      return;
    }
    if (target.kind === "session_error") {
      // 旧版无法确认认证身份时不会建立实时房间，外层按匿名手动刷新模式显示。
      realtimeState.set("manual");
      return;
    }
    if (target.kind === "inactive") {
      // 核心实时四态保持原契约；页头由明确停用目标显示 idle/预览，且不创建 WS。
      realtimeState.set("manual");
      return;
    }
    if (target.kind === "current_error" || target.kind === "identity_error") {
      realtimeState.set("degraded");
      return;
    }
    if (target.kind === "no_target") {
      realtimeState.set(target.reason === "directory_error" ? "degraded" : "manual");
      return;
    }

    const knownAfter = auctionAfter.peek();
    const initialAfter =
      knownAfter !== null && knownAfter[0] === target.auctionId ? knownAfter[1] : 0;
    const cache = createRealtimeCacheAfter(target.auctionId, target.previewRevision, initialAfter);
    const auctionId = target.auctionId;
    const world = target.world;
    let consecutiveFailures = 0;
    for (;;) {
      setConnectionState(realtimeState, "connecting");
      const after = cache.after();
      const onPage = (page: RealtimePage): boolean => {
        // 取消后旧订阅的回调不得再影响页面状态。
        if (!context.alive()) {
          return false;
        }
        const merge = cache.mergePage(page);
        // 目标短暂切到 Checking 再回来时订阅会重启；游标写回外壳，从已确认位置续接，
        // 不从 0 重放同一批事件。
        const nextAfter = cache.after();
        auctionAfter.set([auctionId, nextAfter]);
        // Manual 只由永久错误/重试耗尽进入；普通缺口恢复不应永久粘在 Degraded。
        if (realtimeState.peek() !== "manual") {
          const next = merge.degraded ? "degraded" : "live";
          setConnectionState(realtimeState, next);
        }
        if (merge.refresh_required || merge.bids_changed) {
          // 实时事件已改变服务端权威快照：在唯一的事件处理点整体失效公开读缓存，
          // 避免随后的资源重读命中 TTL 旧值；不按行建立额外订阅。
          invalidateAll();
        }
        if (merge.refresh_required) {
          const nextRefresh = Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER);
          refreshGeneration.set(nextRefresh);
        } else if (merge.bids_changed && documentIsVisible()) {
          // 同页的生命周期/缺口刷新优先；普通竞价只使相关读取资源失效。
          bidRefresh.notify(auctionId);
        }
        return merge.continue_catch_up;
      };
      const attemptStartedMillis = estimatedNow();
      const result = await subscribeAuctionRealtime(
        after,
        auctionId,
        world,
        onPage,
        context.signal,
      );
      if (!context.alive()) {
        return;
      }
      // 会话复核可能推进外壳状态；重连只按连接结束时的存活时长计数。
      const survivedMillis = estimatedNow() - attemptStartedMillis;
      if (!result.ok && result.error.closeCode === REALTIME_SESSION_CLOSE_CODE) {
        requestPublicSessionCheck("refresh");
      }
      if (!context.alive()) {
        return;
      }
      if (!result.ok) {
        if (realtimeErrorIsPermanent(result.error)) {
          // 重连不能自愈：页头降级为手动刷新，不再自动重连。
          console.warn("实时订阅遇到永久错误，转手动刷新态", {
            detail: realtimeErrorDetail(result.error),
          });
          realtimeState.set("manual");
          return;
        }
        // 普通断线按退避策略重连；与 Rust 的 debug 级诊断一致，发布构建不输出。
      }
      if (survivedMillis >= RECONNECT_HEALTHY_MILLIS) {
        consecutiveFailures = 0;
      } else {
        consecutiveFailures = Math.min(consecutiveFailures + 1, 4_294_967_295);
      }
      if (consecutiveFailures >= RECONNECT_MAX_FAILURES) {
        realtimeState.set("manual");
        return;
      }
      setConnectionState(realtimeState, "degraded");
      const delayMillis = reconnectDelayMillis(Math.max(consecutiveFailures, 1));
      await sleepWhileVisible(visibility, delayMillis, context.signal);
      if (!context.alive()) {
        return;
      }
    }
  });

  return null;
}

/**
 * 只在连接状态变化时写入。外壳渲染订阅了该信号，写入同值也会通知订阅者：每个实时事件
 * 都写一次会让整个外壳随每次出价重渲染。
 */
function setConnectionState(state: Cell<LiveConnectionState>, next: LiveConnectionState): void {
  if (state.peek() !== next) {
    state.set(next);
  }
}

/**
 * 已知没有订阅目标时显示页面的终态。需要订阅时等待 readiness 和身份，读取失败显示降级。
 * 停用目标不建立连接：预览期对已登录用户说明开拍后自动连接，匿名用户与竞价期一样是
 * 手动刷新；其余停用场次显示无需实时，不呈现为“未启用”的故障观感。
 */
export function publicConnectionState(
  readiness: PublicReadinessState,
  sessionPending: boolean,
  authenticated: boolean,
  target: PublicRealtimeTarget,
  liveState: LiveConnectionState,
): PublicConnectionState {
  if (readiness === "degraded") {
    return "unavailable";
  }
  if (target.kind === "no_target") {
    switch (target.reason) {
      case "empty":
        return "idle";
      case "directory_error":
        return "degraded";
      case "invalid_auction":
      case "selection_error":
        return "unavailable";
      default:
        return unreachable(target.reason);
    }
  }
  if (readiness === "checking" || sessionPending) {
    return "connecting";
  }
  if (target.kind === "inactive") {
    if (target.preview) {
      return authenticated ? "preview" : "manual";
    }
    return "idle";
  }
  if (target.kind === "current_error" || target.kind === "identity_error") {
    return "degraded";
  }
  if (target.kind === "checking") {
    return "connecting";
  }
  if (!authenticated) {
    return "manual";
  }
  switch (liveState) {
    case "connecting":
      return "connecting";
    case "degraded":
      return "degraded";
    case "live":
      return "live";
    case "manual":
      return "manual";
    default:
      return unreachable(liveState);
  }
}
