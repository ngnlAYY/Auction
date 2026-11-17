// 看板目录与当前场次的对齐重读调度：同一上下文只自动重读一次，读取在途先等待，两侧都来自那次
// 重读仍不对齐才判定用尽。纯判定函数与持有尝试记录、发起失效重读的 hook 放在一起，由
// `AuctionData` 调用；组件调度测试用可控的读取替身驱动真实资源、父组件读取状态与两个刷新代号。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import type { Accessor } from "solid-js";
import { createEffect } from "solid-js";

import { refreshPublicReads } from "./refresh";

/** 一次渲染的对齐重读调度结果。 */
export interface AlignmentReread {
  /** 自动对齐重读已用尽：快照仍不对齐，且场次与目录都已来自那次重读。 */
  readonly exhausted: Accessor<boolean>;
  /** 同一上下文的重读记录（上下文键与目录刷新代号）；用尽面板的重试清掉它，由调度重新发起一次。 */
  readonly attempt: Cell<readonly [string, number] | null>;
}

/** 目录与当前场次对齐判定后的自动重读动作。 */
type AlignmentAction =
  /** 无需动作：已对齐且没有尝试记录，仍有读取在途，本上下文已重读过，或上一轮重读尚未返回。 */
  | "wait"
  /** 已对齐：清掉尝试记录，之后再不对齐时重新获得一次自动重读。 */
  | "clear"
  /** 为当前上下文发起一次失效后的对齐重读。 */
  | "reread";

/**
 * 对齐重读调度 hook。同一上下文只自动对齐重读一次，记录上下文键与重读使用的目录刷新代号；
 * 对齐重读与用尽面板的重试都经共用刷新入口：先失效公开读缓存，再同时重读当前（或历史）场次与
 * 目录，任一侧滞后都能追上，不会命中 TTL 内的新鲜条目而原样返回同一对不一致的快照。
 * `reading` 必须以订阅方式读取场次与目录资源状态，读取结束才会重新判定；其余会变化的条件也用
 * 访问器传入，确保组件函数体只执行一次时仍能重新调度。
 */
export function useAlignmentReread(
  aligned: Accessor<boolean>,
  alignmentKey: Accessor<string>,
  loadedGeneration: Accessor<number | null>,
  reading: Accessor<boolean>,
  lotsRefreshGeneration: Cell<number>,
  refreshGeneration: Cell<number>,
): AlignmentReread {
  const alignmentAttempt = createCell<readonly [string, number] | null>(null);
  // 按值去重的依赖同时包含尝试记录：用尽面板把记录清空后，effect 必须重新判定并重新发起一次。
  const dependency = createValueMemo(() => ({
    aligned: aligned(),
    alignmentKey: alignmentKey(),
    loadedGeneration: loadedGeneration(),
    reading: reading(),
    attempt: alignmentAttempt(),
  }));
  createEffect(dependency, (current) => {
    const action = planAlignmentReread(
      current.aligned,
      current.alignmentKey,
      current.attempt,
      current.loadedGeneration,
      current.reading,
    );
    switch (action) {
      case "wait":
        break;
      case "clear":
        alignmentAttempt.set(null);
        break;
      case "reread": {
        const nextGeneration = Math.min(lotsRefreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER);
        alignmentAttempt.set([current.alignmentKey, nextGeneration]);
        lotsRefreshGeneration.set(nextGeneration);
        refreshPublicReads(refreshGeneration);
        break;
      }
      default:
        return unreachable(action);
    }
  });
  const exhausted = createValueMemo(() =>
    alignmentRetryExhausted(
      aligned(),
      alignmentKey(),
      alignmentAttempt(),
      loadedGeneration(),
      reading(),
    ),
  );
  return { exhausted, attempt: alignmentAttempt };
}

/**
 * 纯判定函数：同一上下文（场次、阶段、开放状态与预览修订号）只自动重读一次。场次或目录仍有
 * 读取在途时先等待：返回后可能就对齐，此时重读只会取消在途读取。上一轮为另一上下文发起的重读
 * 还没返回（目录快照早于那次代号）时同样等待，避免场次读取在一轮之内改变上下文而反复刷新。
 * 仅供测试使用：验证自动对齐重读的纯调度判定。
 */
export function planAlignmentReread(
  aligned: boolean,
  alignmentKey: string,
  attempt: readonly [string, number] | null,
  loadedGeneration: number | null,
  reading: boolean,
): AlignmentAction {
  if (aligned) {
    return attempt === null ? "wait" : "clear";
  }
  if (reading) {
    return "wait";
  }
  if (attempt !== null) {
    const [key, generation] = attempt;
    if (key === alignmentKey) {
      return "wait";
    }
    if (loadedGeneration === null || loadedGeneration < generation) {
      return "wait";
    }
  }
  return "reread";
}

/**
 * 目录快照与当前场次不对齐，且场次与目录都已来自为同一上下文发起的那次对齐重读：自动重读已
 * 用尽。重读刚排定、目录资源仍持有旧快照，或场次、目录仍有读取在途时都不算用尽：任一侧返回
 * 后都可能对齐，避免错误面板一闪而过。
 * 仅供测试使用：验证对齐重读是否已用尽。
 */
export function alignmentRetryExhausted(
  aligned: boolean,
  alignmentKey: string,
  attempt: readonly [string, number] | null,
  loadedGeneration: number | null,
  reading: boolean,
): boolean {
  if (aligned || reading || attempt === null) {
    return false;
  }
  const [key, attemptGeneration] = attempt;
  return key === alignmentKey && loadedGeneration !== null && loadedGeneration >= attemptGeneration;
}
