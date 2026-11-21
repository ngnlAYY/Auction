// 出价记录与履约按已验证的目录分页确认场次；下拉选择需要时再继续读。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { compareText } from "@auction/web-shared/text";
import { createEffect } from "solid-js";

import {
  type AuctionDirectoryObserver,
  fetchBidLogAuctionStatus,
  fetchBidLogAuctions,
  orderBidLogAuctions,
} from "../../../api/queries/bid-log";
import { commitAuctionSelection, type PublicBoardQuery, queryAuctionId } from "../../query";
import { readPublicQueryForState } from "../../query/location";
import { roundReadWorldScope } from "../../query/world-scope";
import { directoryForScope, type ScopeSelection, sameScopeSelection } from "./scope-selection";

interface StatusProgress {
  readonly auctions: readonly AuctionSummaryResponse[];
  readonly complete: boolean;
}

interface DirectoryProgress {
  readonly selection: ScopeSelection;
  readonly generation: number;
  readonly statuses: ReadonlyMap<string, StatusProgress>;
  readonly options: readonly AuctionSummaryResponse[];
  readonly previousOptions: readonly AuctionSummaryResponse[];
  readonly failed: boolean;
}

function couldPrecede(
  progress: StatusProgress,
  preferred: AuctionSummaryResponse | undefined,
  selection: ScopeSelection,
): boolean {
  if (progress.complete) {
    return false;
  }
  const last = progress.auctions.at(-1);
  // 目录按期号、ID 降序。全部世界时主世界优先；后页仍可能出现主世界的首场。
  return (
    preferred === undefined ||
    last === undefined ||
    (selection.scope === "all" && preferred.world_id !== "main") ||
    last.sequence > preferred.sequence ||
    (last.sequence === preferred.sequence && compareText(last.id, preferred.id) > 0)
  );
}

function directorySelection(
  directory: DirectoryProgress,
  statuses: readonly string[],
  requestedId: string,
): AuctionSummaryResponse | null | undefined {
  const found = directory.options.find((auction) => auction.id === requestedId);
  if (found !== undefined) {
    return found;
  }
  const preferred = directory.options[0];
  for (const status of statuses) {
    const progress = directory.statuses.get(status);
    if (
      progress === undefined ||
      (requestedId !== ""
        ? !progress.complete
        : couldPrecede(progress, preferred, directory.selection))
    ) {
      return undefined;
    }
  }
  return preferred ?? null;
}

/** 跨世界和刷新代际隔离目录；只有本轮目录确认过的场次可以启动正文读取。 */
export function useReadRoundDirectory(
  mode: "bidlog" | "fulfillment",
  selection: () => ScopeSelection,
  refreshGeneration: Cell<number>,
  queryState: Cell<PublicBoardQuery>,
  routeStillOnPage: () => boolean,
  orderFulfillment?: (
    auctions: readonly AuctionSummaryResponse[],
  ) => readonly AuctionSummaryResponse[],
) {
  const statuses = mode === "bidlog" ? ["live", "final", "settled"] : ["settled"];
  const auctionPageKeys = mode === "bidlog" ? ["page", "bidlog_page"] : ["page"];
  const selectionKeys = [...auctionPageKeys, "lot"];
  const tasks = createTaskScope();
  const wantedOptions = createCell(false);
  const snapshot = createCell<DirectoryProgress | null>(null);
  const requestedId = createValueMemo(() => queryAuctionId(queryState()) ?? "");
  const invalidAuction = createValueMemo(() => queryState().auction.kind === "invalid");
  const rejected = createCell(false);
  const waiters = new Set<() => void>();
  const wake = () => {
    for (const check of waiters) check();
  };
  createEffect(() => [requestedId(), invalidAuction()] as const, wake);

  const dependencies = createValueMemo(() => [selection(), refreshGeneration()] as const);
  createEffect(dependencies, ([selectedScope, generation]) => {
    const scope = selectedScope.scope;
    // 地址范围无效或已切换时，不能启动目录或沿用旧范围已确认的场次。
    if (
      scope === null ||
      !routeStillOnPage() ||
      roundReadWorldScope(readPublicQueryForState(queryState)) !== scope
    ) {
      wantedOptions.set(false);
      snapshot.set(null);
      return;
    }
    const previous = snapshot.peek();
    if (previous !== null && !sameScopeSelection(previous.selection, selectedScope)) {
      wantedOptions.set(false);
    }
    const previousOptions =
      previous !== null && sameScopeSelection(previous.selection, selectedScope)
        ? previous.options.length === 0
          ? previous.previousOptions
          : previous.options
        : [];
    let current: DirectoryProgress = {
      selection: selectedScope,
      generation,
      statuses: new Map(),
      options: [],
      previousOptions,
      failed: false,
    };
    snapshot.set(current);
    const task = tasks.spawn(async (context) => {
      const observe: AuctionDirectoryObserver = (page, signal) => {
        if (!context.alive() || signal.aborted) {
          return;
        }
        const progress = current.statuses.get(page.status);
        const nextStatuses = new Map(current.statuses);
        nextStatuses.set(page.status, {
          auctions: [...(progress?.auctions ?? []), ...page.auctions],
          complete: page.complete,
        });
        const all = statuses.flatMap((status) => nextStatuses.get(status)?.auctions ?? []);
        const order = selectedScope.scope === "all" ? orderFulfillment : undefined;
        current = {
          ...current,
          statuses: nextStatuses,
          options: (order ?? orderBidLogAuctions)(all),
        };
        snapshot.set(current);
        wake();
        if (page.complete) {
          return;
        }
        return new Promise<void>((resolve) => {
          const check = () => {
            const query = queryState.peek();
            const id = queryAuctionId(query) ?? "";
            const preferred = current.options[0];
            const thisStatus = current.statuses.get(page.status);
            const needed =
              query.auction.kind !== "invalid" &&
              !current.options.some((auction) => auction.id === id) &&
              (id !== ""
                ? thisStatus?.complete !== true
                : current.statuses.size === statuses.length &&
                  thisStatus !== undefined &&
                  couldPrecede(thisStatus, preferred, selectedScope));
            if (
              signal.aborted ||
              ((wantedOptions.peek() || needed) &&
                roundReadWorldScope(readPublicQueryForState(queryState)) === scope)
            ) {
              signal.removeEventListener("abort", check);
              waiters.delete(check);
              resolve();
            }
          };
          waiters.add(check);
          signal.addEventListener("abort", check, { once: true });
          check();
        });
      };
      const world = scope === "all" ? null : scope;
      const result = await (mode === "bidlog"
        ? fetchBidLogAuctions(world, context.signal, observe)
        : fetchBidLogAuctionStatus(world, "settled", context.signal, observe));
      if (!context.alive()) {
        return;
      }
      if (!result.ok) {
        snapshot.set({ ...current, failed: true });
      }
    });
    return () => task.cancel();
  });

  const currentSnapshot = createValueMemo(() => {
    const [scope, generation] = dependencies();
    const current = directoryForScope(snapshot(), scope);
    return current?.generation === generation ? current : null;
  });
  const resolution = createValueMemo(() => {
    // 非法场次仍可加载下拉选项，但要等玩家提交有效目标或清除条件才确认正文。
    if (invalidAuction()) return undefined;
    const current = currentSnapshot();
    return current === null ? undefined : directorySelection(current, statuses, requestedId());
  });
  const correction = createValueMemo(() => {
    const choice = resolution();
    if (choice === undefined) {
      return null;
    }
    const expected = requestedId();
    const next = choice?.id ?? "";
    const current = currentSnapshot();
    return expected === next || current === null
      ? null
      : {
          expected,
          next,
          selection: current.selection,
          generation: current.generation,
        };
  });
  createEffect(correction, (change) => {
    rejected.set(false);
    if (change === null || !routeStillOnPage()) {
      return;
    }
    const current = readPublicQueryForState(queryState);
    if (
      current.auction.kind === "invalid" ||
      (queryAuctionId(current) ?? "") !== change.expected ||
      roundReadWorldScope(current) !== change.selection.scope
    ) {
      return;
    }
    // 场次与第一页同时生效；保留深链的原始 lot 条件供正文门禁检查。
    if (!commitAuctionSelection(queryState, change.next || null, "replace", auctionPageKeys)) {
      rejected.set(true);
    }
  });

  const options = createValueMemo(() => {
    const current = directoryForScope(snapshot(), selection());
    if (current === null) {
      return [];
    }
    const waiting = invalidAuction() ? current.options.length === 0 : resolution() === undefined;
    return waiting && current.previousOptions.length > 0 && !current.failed
      ? current.previousOptions
      : current.options;
  });
  const selected = createValueMemo(
    () => options().find((auction) => auction.id === requestedId()) ?? null,
  );
  const readable = createValueMemo(() => {
    const choice = resolution();
    return choice != null && choice.id === requestedId()
      ? { id: choice.id, world: choice.world_id, generation: refreshGeneration() }
      : null;
  });
  const commitSelection = (id: string | null) =>
    routeStillOnPage() &&
    readPublicQueryForState(queryState).invalidWorld === null &&
    commitAuctionSelection(queryState, id, "push", selectionKeys);
  return {
    options,
    selected,
    readable,
    invalidAuction,
    clearSelection: () => commitSelection(null),
    empty: () => resolution() === null && currentSnapshot()?.failed === false,
    failed: () => resolution() === undefined && currentSnapshot()?.failed === true,
    optionsFailed: () => readable() !== null && currentSnapshot()?.failed === true,
    correctionFailed: () => correction() !== null && rejected(),
    loadOptions() {
      wantedOptions.set(true);
      wake();
    },
    choose(select: HTMLSelectElement) {
      const id = select.value;
      const exists = currentSnapshot()?.options.some((auction) => auction.id === id) === true;
      // 原生下拉先改变选中项；地址写入成功才提交场次、lot 与页码。
      if (!exists || !commitSelection(id)) {
        select.value = selected()?.id ?? "";
      }
    },
  };
}
