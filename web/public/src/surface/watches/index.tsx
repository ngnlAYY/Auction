//! 装备标记的整场读取与身份隔离 owner；所有行共享集合，不逐行请求。

import type { PublicLotWatchesResponse } from "@auction/contracts/dto/public/watch";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { ok } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, For, Show, useContext } from "solid-js";

import type { CommandError } from "../../api/commands";
import { fetchLotWatches } from "../../api/watches";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
} from "..";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { load, save } from "./cache";
import { currentUser, type WatchCommand, watchCommandStore } from "./commands";

/** 完整集合只索引一次，所有目录行按 lot UUID 查状态；false 墓碑保留其真实版本。 */
export type WatchSnapshot = ReadonlyMap<string, readonly [boolean, number]>;

/** 一次整场读取的结果及其发起时的（全局刷新、标记修订、本地失效）修订号。 */
export interface LoadedWatches {
  readonly revision: readonly [number, number, number];
  readonly result: Result<WatchSnapshot | null, CommandError>;
}

/** 标记作用域内所有行共享的读取状态与命令入口。 */
export interface WatchContext {
  readonly userId: string | null;
  readonly auctionId: string;
  readonly session: PublicSessionResource;
  readonly revision: Cell<number>;
  readonly resource: TaskResource<LoadedWatches>;
  readonly cached: Cell<WatchSnapshot | null>;
  readonly commands: Cell<ReadonlyMap<string, WatchCommand>>;
  identityMatches(): boolean;
  ready(): boolean;
  authoritative(lotId: string): readonly [boolean, number] | null;
  displayed(lotId: string): boolean;
  invalidate(): void;
}

/** 整场读取按稳定用户隔离；提交集合和任务由外层页面宿主持有。 */
export const WatchContext = createContext<WatchContext | null>(null);

/** 整场读取按稳定用户隔离；提交集合和任务由外层页面宿主持有。 */
export function LotWatchScope(props: {
  readonly auctionId: string;
  readonly children: JSX.Element;
}): JSX.Element {
  const owner = useContext(PublicCommandOwnerContext);
  const scope = createValueMemo(() => {
    const userId = owner();
    return {
      userId,
      auctionId: props.auctionId,
      key: `${userId ?? ""}:${props.auctionId}`,
    };
  });
  return (
    <For each={[scope()]} keyed={(item) => item.key}>
      {(item) => (
        <ScopedLotWatches userId={item().userId} auctionId={item().auctionId}>
          {props.children}
        </ScopedLotWatches>
      )}
    </For>
  );
}

function ScopedLotWatches(props: {
  readonly userId: string | null;
  readonly auctionId: string;
  readonly children: JSX.Element;
}): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const refresh = useContext(PublicRefreshGenerationContext);
  const commandStore = watchCommandStore();
  const revision = createCell(0);
  const initialPage = props.userId === null ? null : load(props.userId, props.auctionId);
  const cached = createCell<WatchSnapshot | null>(
    initialPage === null ? null : watchSnapshot(initialPage),
  );
  // 只跟随可读身份的变化：会话故障恢复后立即补读，同账号续期不重启整场请求。
  const readable = createValueMemo(
    () => props.userId !== null && currentUser(session()) === props.userId,
  );
  const resource = createTaskResource(
    () => ({
      refresh: refresh(),
      watchRevision: commandStore.revision(),
      localRevision: revision(),
      readable: readable(),
      userId: props.userId,
      auctionId: props.auctionId,
    }),
    async (dependencies, context): Promise<LoadedWatches> => {
      const requestedRevision: readonly [number, number, number] = [
        dependencies.refresh,
        dependencies.watchRevision,
        dependencies.localRevision,
      ];
      if (!dependencies.readable) {
        return { revision: requestedRevision, result: ok(null) };
      }
      const userId = dependencies.userId;
      if (userId === null) {
        return { revision: requestedRevision, result: ok(null) };
      }
      // 请求前后复核身份，不能把会话切换期间的响应写入当前缓存。
      if (currentUser(session.peek()) !== userId) {
        return { revision: requestedRevision, result: ok(null) };
      }
      const result = await fetchLotWatches(userId, dependencies.auctionId, context.signal);
      if (!context.alive() || currentUser(session.peek()) !== userId) {
        return { revision: requestedRevision, result: ok(null) };
      }
      if (!result.ok) {
        return { revision: requestedRevision, result };
      }
      save(result.value);
      const snapshot = watchSnapshot(result.value);
      cached.set(snapshot);
      return { revision: requestedRevision, result: ok(snapshot) };
    },
  );
  // 成功命令要等包含其结果的整场重读成功后才解锁对应 lot；只处理本场条目。
  createEffect(
    () => {
      const loaded = resource();
      if (loaded === undefined || !loaded.result.ok || loaded.result.value === null) {
        return [];
      }
      return settledLots(commandStore.commands(), props.auctionId, loaded.revision[1]);
    },
    (settled) => {
      if (settled.length === 0) {
        return;
      }
      const commands = new Map(commandStore.commands.peek());
      for (const lotId of settled) {
        commands.delete(lotId);
      }
      commandStore.commands.set(commands);
    },
  );

  const identityMatches = () => props.userId !== null && currentUser(session()) === props.userId;

  const context: WatchContext = {
    userId: props.userId,
    auctionId: props.auctionId,
    session,
    revision,
    resource,
    cached,
    commands: commandStore.commands,
    identityMatches,
    ready: () => {
      const loaded = resource();
      return (
        identityMatches() &&
        loaded !== undefined &&
        loaded.revision[2] === revision() &&
        loaded.result.ok &&
        loaded.result.value !== null
      );
    },
    authoritative: (lotId) => {
      if (!context.ready()) {
        return null;
      }
      const loaded = resource();
      if (loaded === undefined || !loaded.result.ok || loaded.result.value === null) {
        return null;
      }
      return loaded.result.value.get(lotId) ?? [false, 0];
    },
    displayed: (lotId) => {
      const pending = context.commands().get(lotId);
      if (pending !== undefined && pending.awaitingRefresh !== null) {
        return pending.payload.watched;
      }
      const authoritative = context.authoritative(lotId);
      if (authoritative !== null) {
        return authoritative[0];
      }
      return context.cached()?.get(lotId)?.[0] ?? false;
    },
    invalidate: () => {
      const next = saturatingAddOne(revision.peek());
      revision.set(next);
    },
  };
  const authenticated = () => context.identityMatches();
  const ready = () => context.ready();
  const failed = () => {
    const loaded = resource();
    return loaded !== undefined && !loaded.result.ok;
  };

  return (
    <WatchContext value={context}>
      <div
        class="public-lot-watches"
        aria-busy={authenticated() && !ready() && !failed() ? "true" : "false"}
      >
        <Show when={authenticated() && !ready() && failed()}>
          <div class="public-lot-watches__sync" role="alert">
            <p>{t("watch-load-error")}</p>
            <button type="button" onClick={() => context.invalidate()}>
              {t("watch-refresh")}
            </button>
          </div>
        </Show>
        {props.children}
      </div>
    </WatchContext>
  );
}

/** 本场可以解锁的成功命令：等待的标记修订号不大于已读集合发起时的修订号，说明该集合是在
 * 命令成功之后读取的，已包含本次结果。忙碌、结果未知或失败的条目没有等待值，保持原样。 */
function settledLots(
  commands: ReadonlyMap<string, WatchCommand>,
  auctionId: string,
  loadedWatchRevision: number,
): string[] {
  const result: string[] = [];
  for (const [lotId, command] of commands) {
    if (
      command.auctionId === auctionId &&
      command.awaitingRefresh !== null &&
      command.awaitingRefresh <= loadedWatchRevision
    ) {
      result.push(lotId);
    }
  }
  return result;
}

function watchSnapshot(page: PublicLotWatchesResponse): WatchSnapshot {
  const snapshot = new Map<string, readonly [boolean, number]>();
  for (const watch of page.watches) {
    snapshot.set(watch.lot_id, [watch.watched, watch.version]);
  }
  return snapshot;
}

function saturatingAddOne(value: number): number {
  return value === Number.MAX_SAFE_INTEGER ? value : value + 1;
}

/** 可选上下文：不在标记作用域内时目录行按未标记处理。 */
export function tryUseWatchContext(): WatchContext | null {
  return useContext(WatchContext);
}

/** 按钮必须位于标记作用域内；缺少上下文是装配错误。 */
export function useWatchContext(): WatchContext {
  const context = useContext(WatchContext);
  if (context === null) {
    throw new Error("LotWatchButton 必须在 LotWatchScope 内渲染");
  }
  return context;
}
