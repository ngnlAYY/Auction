// Public 竞价资源失效与有界合并读取，以及权威读取的统一刷新入口；不修改事件游标或推导竞价
// 结果。
//
// 实时事件只是“竞价发生变化”的通知：这里只递增本地修订号，由各页面资源在 100–250 毫秒的
// 随机窗口后重读自己的权威接口。事件负载中的价格、领先者或版本都不参与页面结果，因此取消
// 合并或丢弃通知只会多读或少读一次快照，不会产生第二种竞价状态。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { useSecondTick } from "@auction/web-shared/platform/clock";
import { sleep } from "@auction/web-shared/platform/timer";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { createContext, createEffect, untrack, useContext } from "solid-js";

import { estimatedNow } from "../../api/clock";
import { parseStrictRfc3339 } from "../../api/session";
import { invalidateAll } from "../../api/swr-cache";
import { PageVisibilityContext } from "../shell/contexts";

/**
 * 合并窗口的基础时长：从首次待处理通知起至少合并 100 毫秒；后续竞价不延后截止时间。
 *
 * “每次出价的页面重读量”即由该窗口约束：窗口内的 N 条竞价通知对同一资源只产生一次重读，
 * 窗口外的通知各自产生一次。HTTP 路由计数用于在 Final 阶段实测该比值，口径见
 * `crates/auction-observability/README.md`。
 * 导出仅供测试核对合并窗口的边界。
 */
export const BID_REFRESH_COALESCE_MILLIS = 100;
/**
 * 每个窗口另加的均匀抖动上界（不含）：同一事件推给全部连接，固定窗口会让所有客户端在同一时刻
 * 重读并一起撞上匿名读闸门的并发许可上限；抖动把重读打散到 100–250 毫秒。
 * 导出仅供测试核对合并窗口的边界。
 */
export const BID_REFRESH_JITTER_MILLIS = 150;

/**
 * 由 `[0, 1)` 的均匀随机样本计算本次合并窗口，结果总在 `[100, 250)` 毫秒内。
 * 随机源由调用方注入（浏览器为 `Math.random`）；越界或非有限样本按边界处理，不会越出窗口。
 * 仅供测试使用：验证合并窗口的边界与抖动取值。
 */
export function coalesceWindowMillis(sample: number): number {
  const bounded = Number.isFinite(sample) ? Math.min(Math.max(sample, 0), 1) : 0;
  // 样本已截到 [0, 1]：乘积落在 [0, 150]，截断不会溢出；1.0 再收进上界之内。
  const jitter = Math.trunc(bounded * BID_REFRESH_JITTER_MILLIS);
  return BID_REFRESH_COALESCE_MILLIS + Math.min(jitter, BID_REFRESH_JITTER_MILLIS - 1);
}

/**
 * 外壳权威轮询、恢复可见、开场迁移、看板对齐重读与错误重试共用的刷新入口：先使公开读缓存
 * 过期，再推进外壳的全局刷新代号。只推进代号时，TTL 内的重读直接命中新鲜条目，等于没有重读；
 * 失效后的重读带旧 ETag 条件请求，正文未变时 304 复用，同一 URL 的在途读取仍单飞合并。
 */
export function refreshPublicReads(refreshGeneration: Cell<number>): void {
  invalidateAll();
  const nextGeneration = Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER);
  refreshGeneration.set(nextGeneration);
}

/** 同一场次的开场补读归公开看板持有，账号切换不重新触发。 */
export function useAuctionOpeningRefresh(
  auction: () => CurrentAuctionItemResponse | null,
  historical: () => boolean,
  refreshGeneration: Cell<number>,
): void {
  const tick = useSecondTick();
  const visibility = useContext(PageVisibilityContext);
  const opening = createValueMemo(() => {
    const round = auction();
    return round === null
      ? null
      : {
          key: `${round.id}:${round.opens_at}`,
          opensAt: parseStrictRfc3339(round.opens_at),
          shouldRefresh: !historical() && round.presentation_mode !== "live",
        };
  });
  let openingKey: string | null = null;
  let refreshed = false;
  createEffect(
    () => {
      const current = opening();
      return { current, tick: current?.shouldRefresh ? tick() : null };
    },
    ({ current }) => {
      const key = current?.key ?? null;
      if (key !== openingKey) {
        openingKey = key;
        refreshed = false;
      }
      // 预览没有 WebSocket，开场时先失效 TTL 内的旧快照，再推进权威读取代号。
      if (
        current?.shouldRefresh &&
        !refreshed &&
        visibility.isVisible() &&
        current.opensAt !== null &&
        BigInt(estimatedNow()) * 1_000_000n >= current.opensAt
      ) {
        refreshed = true;
        refreshPublicReads(refreshGeneration);
      }
    },
  );
}

/** Public 外壳持有的竞价失效修订号，与生命周期和人工刷新的全局修订号分离。 */
export interface PublicBidRefresh {
  /** 单调递增的竞价失效修订号。 */
  readonly revision: Cell<number>;
  /** 推进修订号；外壳把它作为上下文提供给全部竞价资源。 */
  notify(): void;
}

/** 外壳把竞价失效修订号放入此上下文，供 `useBidRefresh` 订阅。 */
export const PublicBidRefreshContext = createContext<PublicBidRefresh>();

/** 外壳把全局刷新代号放入此上下文，供各页面读取并在重启资源时使用。 */
export const PublicRefreshGenerationContext = createContext<Cell<number>>();

/**
 * 单次竞价通知在合并窗口内的调度判定。
 *
 * 与信号和计时器解耦，保证以下性质可直接测试，而不是只靠组件行为推断：
 * 未消费的修订号只排定一次；窗口等待中的通知既不延长截止也不叠加读取；资源未就绪时
 * 不消费修订号，等它就绪后仍会读取最新快照。
 */
export type BidRefreshDecision =
  /** 修订号已消费（或通知已被禁用分支吸收）：不动作。 */
  | "idle"
  /** 资源尚未 `ready`：本次不排定、不消费，避免取消进行中的请求。 */
  | "wait_for_ready"
  /** 已有窗口在等待：合并进该窗口，不延长其截止时间。 */
  | "coalescing"
  /** 为当前修订号排定一个合并窗口。 */
  | { readonly kind: "schedule"; readonly revision: number };

/** 纯判定函数：只依赖修订号与资源状态，不读事件负载、不产生副作用。 */
export function planBidRefresh(
  currentRevision: number,
  consumedRevision: number,
  scheduled: boolean,
  ready: boolean,
): BidRefreshDecision {
  if (currentRevision === consumedRevision) {
    return "idle";
  }
  if (scheduled) {
    return "coalescing";
  }
  if (!ready) {
    return "wait_for_ready";
  }
  return { kind: "schedule", revision: currentRevision };
}

/**
 * 仅刷新已挂载且启用的竞价资源；进行中的读取先完成，再补读尚未消费的通知。
 * 竞价修订号不能直接成为资源的依赖，否则持续竞价会反复取消慢请求。
 * 与资源在同一作用域调用：延后任务与已消费修订号一起存活，卸载时由新资源自行读取权威快照。
 */
export function useBidRefresh<T>(resource: TaskResource<T>, enabled: () => boolean): void {
  const refresh = useContext(PublicBidRefreshContext);
  // 与源码的 use_reactive 一样按值去重；关闭后必须立即退订，不再随每次竞价执行空刷新。
  const enabledValue = createValueMemo(enabled);
  const consumed = createCell(refresh.revision.peek());
  const scheduled = createCell(false);
  const tasks = createTaskScope();

  createEffect(
    () => {
      if (!enabledValue()) {
        return { enabled: false } as const;
      }
      return {
        enabled: true,
        revision: refresh.revision(),
        // 先读资源状态以订阅它就绪/重新挂起的变化，再按纯函数决定是否排定。
        state: resource.state(),
      } as const;
    },
    (current) => {
      if (!current.enabled) {
        // 闭合的代理面板不订阅竞价或资源状态，避免每行随通知执行空刷新。
        consumed.set(refresh.revision.peek());
        return;
      }
      const decision = planBidRefresh(
        current.revision,
        consumed.peek(),
        scheduled.peek(),
        current.state === "ready",
      );
      if (decision === "idle" || decision === "coalescing" || decision === "wait_for_ready") {
        return;
      }
      scheduled.set(true);
      // 每个窗口各取一次随机样本；抖动只改变重读时刻，不改变合并与消费规则。
      const windowMillis = coalesceWindowMillis(Math.random());
      tasks.spawn(async (context) => {
        await sleep(windowMillis, context.signal);
        if (!context.alive()) {
          return;
        }
        scheduled.set(false);
        const revision = refresh.revision.peek();
        // 等待期间可能切换查询、关闭代理面板或触发全局刷新；只重启空闲资源。
        if (
          untrack(enabledValue) &&
          revision !== consumed.peek() &&
          untrack(() => resource.state()) === "ready"
        ) {
          consumed.set(revision);
          resource.restart();
        }
      });
    },
  );
}
