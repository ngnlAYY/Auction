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
import { createContext, createEffect, For, Show, untrack, useContext } from "solid-js";

import type { CommandError } from "../../api/commands";
import { startPublicSessionIdentityObservation } from "../../api/session/revision";
import { fetchLotWatches } from "../../api/watches";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
} from "..";
import { PublicRefreshGenerationContext } from "../board/refresh";
import {
  currentSessionConfirmation,
  type PublicSessionAuthority,
} from "../shell/session-authority";
import { load, save } from "./cache";
import { currentUser, type WatchCommand, watchCommandStore } from "./commands";

/** 完整集合只索引一次，所有目录行按 lot UUID 查状态；false 墓碑保留其真实版本。 */
export type WatchSnapshot = ReadonlyMap<string, readonly [boolean, number]>;

/** 一次整场读取的结果及其发起时的（全局刷新、标记修订、本地失效）修订号。 */
export interface LoadedWatches {
  readonly revision: readonly [number, number, number];
  readonly sessionRevision: string | null;
  readonly result: Result<
    {
      readonly page: PublicLotWatchesResponse;
      readonly snapshot: WatchSnapshot;
      readonly observation: ReturnType<typeof startPublicSessionIdentityObservation>;
    } | null,
    CommandError
  >;
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
  const session: PublicSessionAuthority = useContext(PublicSessionResourceContext);
  const refresh = useContext(PublicRefreshGenerationContext);
  const commandStore = watchCommandStore();
  const revision = createCell(0);
  const initialPage = props.userId === null ? null : load(props.userId, props.auctionId);
  const cached = createCell<WatchSnapshot | null>(
    initialPage === null ? null : watchSnapshot(initialPage),
  );
  const accepted = createCell<LoadedWatches | null>(null);
  // 只跟随可读身份的变化：会话故障恢复后立即补读，同账号续期不重启整场请求。
  const readable = createValueMemo(
    () => props.userId !== null && currentUser(session()) === props.userId,
  );
  const resource = createTaskResource(
    () => ({
      refresh: refresh(),
      watchRevision: commandStore.revision(),
      localRevision: revision(),
      confirmation: session.confirmation?.() ?? null,
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
      const userId = dependencies.userId;
      const observation = startPublicSessionIdentityObservation();
      const current = () => currentUser(session.peek()) === userId && observation.cookieCurrent();
      const requested = {
        revision: requestedRevision,
        sessionRevision: observation.primaryRevision,
      };
      // 请求前后复核身份，不能把会话切换期间的响应写入当前缓存。
      if (
        !dependencies.readable ||
        userId === null ||
        dependencies.confirmation === null ||
        dependencies.confirmation.revision !== observation.primaryRevision ||
        !current()
      ) {
        return { ...requested, result: ok(null) };
      }
      const result = await fetchLotWatches(userId, dependencies.auctionId, context.signal, {
        current,
      });
      if (!context.alive() || !current()) {
        return { ...requested, result: ok(null) };
      }
      if (!result.ok) {
        return { ...requested, result };
      }
      return {
        ...requested,
        result: ok({ page: result.value, snapshot: watchSnapshot(result.value), observation }),
      };
    },
  );
  const identityMatches = () => props.userId !== null && currentUser(session()) === props.userId;
  const revisionMatches = (loaded: LoadedWatches): boolean =>
    currentSessionConfirmation(session)?.revision === loaded.sessionRevision &&
    loaded.revision[2] === revision();

  createEffect(resource, (loaded) => {
    if (loaded === undefined || !loaded.result.ok || loaded.result.value === null) {
      return;
    }
    const { page, snapshot, observation } = loaded.result.value;
    const current = () =>
      observation.cookieCurrent() && untrack(() => identityMatches() && revisionMatches(loaded));
    // 只有资源实际交付且仍属于当前会话代次的结果，才能回填内存或持久快照。
    if (!current()) {
      return;
    }
    save(page, observation.cacheRevision());
    if (!current()) {
      return;
    }
    cached.set(snapshot);
    accepted.set(loaded);
    // 同一代次的权威重读成功后，才解锁本场成功命令。
    const stored = commandStore.commands.peek();
    const settled = settledLots(stored, page.auction_id, loaded.revision[1]);
    if (settled.length > 0) {
      const commands = new Map(stored);
      for (const lotId of settled) {
        commands.delete(lotId);
      }
      commandStore.commands.set(commands);
    }
  });

  const authoritativeSnapshot = (): WatchSnapshot | null => {
    const loaded = resource();
    if (
      !identityMatches() ||
      loaded === undefined ||
      loaded !== accepted() ||
      !revisionMatches(loaded) ||
      !loaded.result.ok
    ) {
      return null;
    }
    return loaded.result.value?.snapshot ?? null;
  };

  const context: WatchContext = {
    userId: props.userId,
    auctionId: props.auctionId,
    session,
    revision,
    resource,
    cached,
    commands: commandStore.commands,
    identityMatches,
    ready: () => authoritativeSnapshot() !== null,
    authoritative: (lotId) => {
      const snapshot = authoritativeSnapshot();
      return snapshot === null ? null : (snapshot.get(lotId) ?? [false, 0]);
    },
    displayed: (lotId) => {
      const pending = context.commands().get(lotId);
      if (pending !== undefined && pending.awaitingRefresh !== null) {
        return pending.payload.watched;
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
 * 命令成功之后读取的，已包含本次结果。忙碌、结果未知或失败的条目没有等待值，保持原样。
 * 仅供测试使用。 */
export function settledLots(
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
