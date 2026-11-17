// 看板场次与目录的对齐重读：同一上下文一次、读取在途先等待，两侧重读返回后才判定用尽。

import { expect, onTestFinished, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { TaskContext } from "@auction/web-shared/reactive/tasks";
import { createEffect, createRoot, flush, getOwner, runWithOwner } from "solid-js";

import { alignmentRetryExhausted, planAlignmentReread, useAlignmentReread } from "./alignment";
import { refreshPublicReads } from "./refresh";

test("alignment_is_exhausted_only_after_the_retry_snapshot_arrives：只有对齐重读快照返回后才判定用尽", () => {
  const attempt = ["auction:live", 3] as const;
  // 对齐时从不算用尽。
  expect(alignmentRetryExhausted(true, "auction:live", attempt, 3, false)).toBe(false);
  // 尚未发起对齐重读。
  expect(alignmentRetryExhausted(false, "auction:live", null, 2, false)).toBe(false);
  // 重读已排定，但资源仍持有排定前的旧快照：不闪错误面板。
  expect(alignmentRetryExhausted(false, "auction:live", attempt, 2, false)).toBe(false);
  // 读取失败由错误分支处理，这里没有已加载的代号。
  expect(alignmentRetryExhausted(false, "auction:live", attempt, null, false)).toBe(false);
  // 上下文已切换：旧尝试不算数，等待 effect 为新上下文重读。
  expect(alignmentRetryExhausted(false, "auction:final", attempt, 3, false)).toBe(false);
  // 那次对齐重读的快照仍不对齐（例如 503 时缓存回退的旧快照）：用尽，显示可重试的错误。
  expect(alignmentRetryExhausted(false, "auction:live", attempt, 3, false)).toBe(true);
  expect(alignmentRetryExhausted(false, "auction:live", attempt, 4, false)).toBe(true);
});

test("exhaustion_also_waits_for_reads_still_in_flight：读取仍在途时即使目录已返回也不判定用尽", () => {
  // 目录已按本轮代号返回，但当前（或历史深链的）场次或目录仍有读取在途：返回后可能就对齐，
  // 不算用尽。
  const attempt = ["auction:preview:3", 5] as const;
  expect(alignmentRetryExhausted(false, "auction:preview:3", attempt, 5, true)).toBe(false);
  expect(alignmentRetryExhausted(false, "auction:preview:3", attempt, 5, false)).toBe(true);
});

test("each_misaligned_context_gets_exactly_one_reread：每个不对齐上下文只自动重读一次", () => {
  // 已对齐：没有尝试时无需动作；有尝试时清掉，之后再不对齐重新获得一次重读。
  expect(planAlignmentReread(true, "a:live", null, 1, false)).toBe("wait");
  const attempt = ["a:live", 2] as const;
  expect(planAlignmentReread(true, "a:live", attempt, 2, false)).toBe("clear");
  // TTL 内旧场次配新目录（或相反）首次不对齐：读取都已返回时发起一次失效后的重读。
  expect(planAlignmentReread(false, "a:live", null, 1, false)).toBe("reread");
  // 仍有读取在途（例如轮询时目录先返回、场次还在读）：返回后可能就对齐，先不重读。
  expect(planAlignmentReread(false, "a:live", null, 1, true)).toBe("wait");
  // 同一上下文已重读过：不再自动重读（有界），用尽后只由用户重试清掉记录再发起。
  for (const [loaded, reading] of [
    [1, false],
    [2, false],
    [2, true],
    [null, false],
  ] as const) {
    expect(planAlignmentReread(false, "a:live", attempt, loaded, reading)).toBe("wait");
  }
});

test("a_new_context_waits_until_the_previous_round_has_returned：新上下文等上一轮重读返回后再重读", () => {
  // 上一轮为预览上下文发起（代号 4）；场次重读返回后上下文已切到开场后的 live（切换世界或
  // 场次时同理：旧上下文的尝试不算新上下文的重读，也不让新上下文判定用尽）。
  const attempt = ["a:preview:1", 4] as const;
  // 目录仍是上一轮之前的快照，或仍有读取在途：先等待，不取消在途读取、不反复刷新。
  expect(planAlignmentReread(false, "a:live:1", attempt, 3, false)).toBe("wait");
  expect(planAlignmentReread(false, "a:live:1", attempt, 4, true)).toBe("wait");
  // 上一轮已全部返回仍不对齐：为新上下文发起它自己的一次重读。
  expect(planAlignmentReread(false, "a:live:1", attempt, 4, false)).toBe("reread");
});

type Side = "auction" | "lots";

interface Frame {
  readonly aligned: boolean;
  readonly reading: boolean;
  readonly exhausted: boolean;
}

interface ReadSlot {
  readonly side: Side;
  readonly promise: Promise<string>;
  dropped: boolean;
  replied: boolean;
  resolve(value: string): void;
}

interface Script {
  readonly reads: ReadSlot[];
  readonly frames: Frame[];
}

interface AlignmentHarness {
  refresh(): Promise<void>;
  reply(side: Side, value: string): Promise<void>;
  started(): readonly [number, number];
  frames(): readonly Frame[];
  lastFrame(): Frame;
  dispose(): void;
}

function slot(side: Side): ReadSlot {
  let resolveValue: (value: string) => void = () => undefined;
  const promise = new Promise<string>((resolve) => {
    resolveValue = resolve;
  });
  return {
    side,
    promise,
    dropped: false,
    replied: false,
    resolve(value) {
      resolveValue(value);
    },
  };
}

function read(script: Script, side: Side, context: TaskContext): Promise<string> {
  const current = slot(side);
  script.reads.push(current);
  context.signal.addEventListener(
    "abort",
    () => {
      current.dropped = true;
    },
    { once: true },
  );
  return current.promise;
}

async function settle(): Promise<void> {
  await Promise.resolve();
  flush();
  await Promise.resolve();
  flush();
}

async function reply(script: Script, side: Side, value: string): Promise<void> {
  const current = script.reads.findLast(
    (readSlot) => readSlot.side === side && !readSlot.dropped && !readSlot.replied,
  );
  if (current === undefined) {
    throw new Error(`${side} 没有在途读取`);
  }
  current.replied = true;
  current.resolve(value);
  await settle();
}

function lastFrame(script: Script): Frame {
  const frame = script.frames.at(-1);
  if (frame === undefined) {
    throw new Error("目录尚未渲染");
  }
  return frame;
}

async function mounted(initial: string): Promise<AlignmentHarness> {
  const script: Script = { reads: [], frames: [] };
  let dispose: () => void = () => undefined;
  const environment = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    const owner = getOwner();
    const refreshGeneration = createCell(0);
    const auction = createTaskResource(
      () => refreshGeneration(),
      async (_dependencies, context) => read(script, "auction", context),
    );

    const createCatalog = (): void => {
      const lotsRefreshGeneration = createCell(0);
      const lotsDependencies = createValueMemo(
        () => [refreshGeneration(), lotsRefreshGeneration()] as const,
      );
      const lots = createTaskResource(
        lotsDependencies,
        async ([, generation], context) =>
          [generation, await read(script, "lots", context)] as const,
      );
      const aligned = () => {
        const auctionValue = auction();
        if (auctionValue === undefined) {
          return true;
        }
        const loaded = lots();
        return loaded === undefined || loaded[1] === auctionValue;
      };
      const loadedGeneration = () => lots()?.[0] ?? null;
      const reading = () => auction.state() === "pending" || lots.state() === "pending";
      const alignment = useAlignmentReread(
        aligned,
        () => auction() ?? "",
        loadedGeneration,
        reading,
        lotsRefreshGeneration,
        refreshGeneration,
      );
      createEffect(
        () => ({
          aligned: aligned(),
          reading: reading(),
          exhausted: alignment.exhausted(),
        }),
        (frame) => {
          script.frames.push(frame);
        },
      );
    };

    return { owner, refreshGeneration, createCatalog };
  });

  flush();
  await settle();
  await reply(script, "auction", initial);
  if (environment.owner === null) {
    throw new Error("看板测试宿主缺少 owner");
  }
  runWithOwner(environment.owner, environment.createCatalog);
  flush();
  await settle();
  await reply(script, "lots", initial);
  expect(lastFrame(script)).toEqual({ aligned: true, reading: false, exhausted: false });

  return {
    async refresh() {
      refreshPublicReads(environment.refreshGeneration);
      await settle();
    },
    reply(side, value) {
      return reply(script, side, value);
    },
    started() {
      const count = (side: Side) =>
        script.reads.filter((readSlot) => readSlot.side === side).length;
      return [count("auction"), count("lots")] as const;
    },
    frames() {
      return script.frames;
    },
    lastFrame() {
      return lastFrame(script);
    },
    dispose,
  };
}

test("auction_returning_first_waits_for_the_inflight_catalog：场次先返回时等待在途目录", async () => {
  const harness = await mounted("preview");
  onTestFinished(harness.dispose);
  await harness.refresh();
  await harness.reply("auction", "live");
  expect(harness.lastFrame()).toEqual({ aligned: false, reading: true, exhausted: false });

  await harness.reply("lots", "live");

  expect(harness.lastFrame()).toEqual({ aligned: true, reading: false, exhausted: false });
  expect(harness.started()).toEqual([2, 2]);
  expect(harness.frames().every((frame) => !frame.exhausted)).toBe(true);
});

test("catalog_returning_first_waits_for_the_inflight_auction：目录先返回时等待在途场次", async () => {
  const harness = await mounted("preview");
  onTestFinished(harness.dispose);
  await harness.refresh();
  await harness.reply("lots", "live");
  expect(harness.lastFrame()).toEqual({ aligned: false, reading: true, exhausted: false });

  await harness.reply("auction", "live");

  expect(harness.lastFrame()).toEqual({ aligned: true, reading: false, exhausted: false });
  expect(harness.started()).toEqual([2, 2]);
  expect(harness.frames().every((frame) => !frame.exhausted)).toBe(true);
});

test("context_switch_during_the_reread_gets_one_reread_and_then_aligns：重读期间切换上下文只为新上下文再重读一次", async () => {
  const harness = await mounted("preview");
  onTestFinished(harness.dispose);
  await harness.refresh();
  await harness.reply("auction", "live");
  await harness.reply("lots", "preview");
  expect(harness.started()).toEqual([3, 3]);

  await harness.reply("auction", "final");
  expect(harness.lastFrame()).toEqual({ aligned: false, reading: true, exhausted: false });
  expect(harness.started()).toEqual([3, 3]);
  await harness.reply("lots", "live");
  expect(harness.started()).toEqual([4, 4]);

  await harness.reply("auction", "final");
  await harness.reply("lots", "final");

  expect(harness.lastFrame()).toEqual({ aligned: true, reading: false, exhausted: false });
  expect(harness.started()).toEqual([4, 4]);
  expect(harness.frames().every((frame) => !frame.exhausted)).toBe(true);
});

test("exhaustion_is_reported_only_after_both_reread_sides_return：两侧重读都返回后才报告用尽", async () => {
  const harness = await mounted("preview");
  onTestFinished(harness.dispose);
  await harness.refresh();
  await harness.reply("auction", "live");
  await harness.reply("lots", "preview");
  expect(harness.started()).toEqual([3, 3]);

  await harness.reply("lots", "preview");
  expect(harness.lastFrame()).toEqual({ aligned: false, reading: true, exhausted: false });

  await harness.reply("auction", "live");

  expect(harness.lastFrame()).toEqual({ aligned: false, reading: false, exhausted: true });
  expect(harness.started()).toEqual([3, 3]);
});
