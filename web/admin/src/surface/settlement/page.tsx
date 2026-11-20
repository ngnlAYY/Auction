// 结算与恢复页面：目录、详情加载与恢复确认。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, onCleanup, Show, untrack } from "solid-js";

import { type AdminRecoveryReport, fetchAdminRecovery } from "../../api/recovery";
import { type AdminSettlementReport, fetchAdminSettlement } from "../../api/settlement";
import {
  type AdminSettlementCatalogEntry,
  type AdminSettlementCatalogPage,
  fetchAdminSettlementCatalog,
} from "../../api/settlement-catalog";
import { useAdminSessionHost } from "../auth/shell";
import { useAdminPagination } from "../pagination";
import { usePendingCommandGate } from "../pending";
import { createCommandReload, usePendingCommandsWithGate } from "../pending/sources";
import { type AdminActionFeedback, adminApiErrorMessage, adminWorldLabel } from "../presentation";
import {
  type CatalogLoad,
  type CatalogLookupLoad,
  type CatalogSelection,
  type DetailLoad,
  generationNumberCell,
  matchingLookup,
  reconcileSettlementCatalog,
  reportForSelection,
  resetDetailState,
  resolveCatalogSelection,
  type SettlementDetail,
} from "./page-model";
import { AdminSettlementPageView, type AdminSettlementViewState } from "./page-view";
import {
  readSettlementQuery,
  SETTLEMENT_PAGE_SIZE,
  type SettlementQuery,
  useSettlementLocationRevision,
} from "./query";
import type { RecoveryConfirmationDraft, RecoveryRead } from "./recovery";

export { resetDetailState };

/** 加载并展示已结算拍卖的多条件报表页面。 */
export function page(): JSX.Element {
  const locationRevision = useSettlementLocationRevision();
  const reloadGeneration = createCommandReload(0n);
  const initialQuery = createValueMemo((): SettlementQuery | null => {
    locationRevision();
    const query = readSettlementQuery();
    return query.ok ? query.value : null;
  });

  return (
    <Show
      when={initialQuery()}
      fallback={
        <section
          class="admin-auction-page admin-settlement-page"
          aria-labelledby="settlement-title"
        >
          <h1 id="settlement-title">{t("page-settlement-title")}</h1>
          <AsyncState kind="error" title={t("admin-settlement-invalid-link")} detail="" />
        </section>
      }
    >
      {(query) => (
        <AdminSettlementPage
          initialQuery={query()}
          locationRevision={locationRevision}
          reloadGeneration={reloadGeneration}
        />
      )}
    </Show>
  );
}

interface AdminSettlementPageProps {
  readonly initialQuery: SettlementQuery;
  readonly locationRevision: Cell<number>;
  readonly reloadGeneration: Cell<bigint>;
}

function AdminSettlementPage(props: AdminSettlementPageProps): JSX.Element {
  const host = useAdminSessionHost();
  // `<Show>` 的初值是响应式 prop；组件只把它当作挂载快照，后续由地址恢复 effect 更新。
  const initialQuery = untrack(() => props.initialQuery);
  const worldFilter = createCell(initialQuery.worldId);
  const selectedAuctionId = createCell(initialQuery.auctionId);
  const requestedSequence = createCell<number | null>(initialQuery.sequence);
  const catalogPage = createCell(initialQuery.page);
  const appliedLocationRevision = createCell(props.locationRevision.peek());
  const catalogPagination = useAdminPagination(
    "settlement-catalog",
    [SETTLEMENT_PAGE_SIZE],
    100,
    100_000,
  );
  const catalogPageSize = catalogPagination.sizeSignal();
  const lotPage = createCell(1);
  const recoveryPage = createCell(1);
  const reloadGenerationNumber = generationNumberCell(props.reloadGeneration);
  const recoveryReloadGeneration = createCell(0);
  const confirmation = createCell<RecoveryConfirmationDraft | null>(null);
  const reason = createCell("");
  const actionFeedback = createCell<AdminActionFeedback | null>(null);
  const selectionGeneration = createCell(0);
  // 已提交命令由会话宿主持有；页面销毁后冲突回执不得再推进本页的读取世代。
  let alive = true;
  onCleanup(() => {
    alive = false;
  });

  // 浏览器返回/前进可能只改变 query；按地址恢复筛选并关闭旧报告上的操作上下文。
  createEffect(
    () => props.locationRevision(),
    (revision) => {
      const query = readSettlementQuery();
      if (!query.ok) {
        return;
      }
      const unchanged =
        worldFilter.peek() === query.value.worldId &&
        selectedAuctionId.peek() === query.value.auctionId &&
        requestedSequence.peek() === query.value.sequence &&
        catalogPage.peek() === query.value.page;
      if (!unchanged) {
        worldFilter.set(query.value.worldId);
        selectedAuctionId.set(query.value.auctionId);
        requestedSequence.set(query.value.sequence);
        catalogPage.set(query.value.page);
        resetDetailState(
          lotPage,
          recoveryPage,
          confirmation,
          reason,
          actionFeedback,
          selectionGeneration,
        );
      }
      appliedLocationRevision.set(revision);
    },
  );

  const catalog = createTaskResource(
    () => {
      props.reloadGeneration();
      return {
        worldId: worldFilter(),
        page: catalogPage(),
        pageSize: catalogPageSize(),
      };
    },
    async (deps, context): Promise<CatalogLoad> => {
      const result = await fetchAdminSettlementCatalog(
        host,
        deps.worldId,
        null,
        deps.page,
        deps.pageSize,
        context.signal,
      );
      return {
        worldId: deps.worldId,
        page: deps.page,
        pageSize: deps.pageSize,
        result,
      };
    },
  );
  // URL 中的期号可能不在当前目录页；单独使用服务端筛选解析它，同时保留完整分页目录供切换。
  // 只有当前目录页已加载且页内找不到请求的期号时才查找：默认选中与下拉选择的期号都在页内，
  // 不再多发一次结果用不到的请求；深链接目标不在当前页时，查找在目录页返回后发出。
  const lookupSequence = createValueMemo((): number | null => {
    const sequence = requestedSequence();
    if (sequence === null) {
      return null;
    }
    const load = catalog();
    if (load === undefined) {
      return null;
    }
    const currentPage =
      load.worldId === worldFilter() &&
      load.page === catalogPage() &&
      load.pageSize === catalogPageSize();
    if (!currentPage || !load.result.ok) {
      return null;
    }
    const resolution = resolveCatalogSelection(
      load.result.value,
      null,
      selectedAuctionId(),
      sequence,
    );
    return resolution.kind === "resolving" ? sequence : null;
  });
  const catalogLookup = createTaskResource(
    () => ({ worldId: worldFilter(), sequence: lookupSequence() }),
    async (deps, context): Promise<CatalogLookupLoad> => {
      let result: Result<AdminSettlementCatalogPage | null, string>;
      if (deps.sequence === null) {
        result = ok(null);
      } else {
        const fetched = await fetchAdminSettlementCatalog(
          host,
          deps.worldId,
          deps.sequence,
          1,
          10,
          context.signal,
        );
        result = fetched.ok ? ok(fetched.value) : err(fetched.error);
      }
      return { worldId: deps.worldId, sequence: deps.sequence, result };
    },
  );
  const settlement = createTaskResource(
    () => ({ auctionId: selectedAuctionId(), reload: props.reloadGeneration() }),
    async (deps, context): Promise<DetailLoad<AdminSettlementReport>> => {
      let result: Result<AdminSettlementReport | null, string>;
      if (deps.auctionId === "") {
        result = ok(null);
      } else {
        const fetched = await fetchAdminSettlement(host, deps.auctionId, context.signal);
        result = fetched.ok ? ok(fetched.value) : err(fetched.error);
      }
      return { auctionId: deps.auctionId, result };
    },
  );
  const recovery = createTaskResource(
    () => ({
      generation: [reloadGenerationNumber(), recoveryReloadGeneration()] as const,
      auctionId: selectedAuctionId(),
    }),
    async (
      deps,
      context,
    ): Promise<readonly [readonly [number, number], DetailLoad<AdminRecoveryReport>]> => {
      let result: Result<AdminRecoveryReport | null, string>;
      if (deps.auctionId === "") {
        result = ok(null);
      } else {
        const fetched = await fetchAdminRecovery(host, deps.auctionId, context.signal);
        result = fetched.ok ? ok(fetched.value) : err(fetched.error);
      }
      return [deps.generation, { auctionId: deps.auctionId, result }] as const;
    },
  );
  const commandGate = usePendingCommandGate();
  const pending = usePendingCommandsWithGate(props.reloadGeneration, commandGate, (entity) => {
    const prefix = "settlement-recovery:";
    if (!entity.startsWith(prefix)) {
      return false;
    }
    const recoveryId = entity.slice(prefix.length);
    const selected = selectedAuctionId.peek();
    if (selected === "") {
      return false;
    }
    const state = recovery.peek();
    if (state === undefined) {
      return true;
    }
    const load = state[1];
    if (load.auctionId !== selected || !load.result.ok) {
      return true;
    }
    const report = load.result.value;
    return report === null || report.recoveries.some((entry) => entry.id === recoveryId);
  });
  // 派生值供恢复报告和确认框订阅，避免命令忙碌时重绘整页。
  const actionBusy = createValueMemo(() => pending.anyBusy());

  // 每个筛选页都以服务端稳定排序的第一项为默认值；第一页因此始终选择最后结算的一期。
  createEffect(
    () => ({
      catalog: catalog(),
      world: worldFilter(),
      page: catalogPage(),
      pageSize: catalogPageSize(),
      selected: selectedAuctionId(),
      sequence: requestedSequence(),
      lookup: catalogLookup(),
      appliedLocationRevision: appliedLocationRevision(),
    }),
    (deps) => {
      const load = deps.catalog;
      if (load === undefined) {
        return;
      }
      reconcileSettlementCatalog({
        load,
        appliedLocationRevision: deps.appliedLocationRevision,
        locationRevision: props.locationRevision,
        currentWorld: deps.world,
        currentPage: deps.page,
        currentPageSize: deps.pageSize,
        currentId: deps.selected,
        desiredSequence: deps.sequence,
        lookup: deps.lookup ?? null,
        worldFilter,
        catalogPage,
        catalogPageSize,
        selectedAuctionId,
        requestedSequence,
        lotPage,
        recoveryPage,
        confirmation,
        reason,
        actionFeedback,
        selectionGeneration,
      });
    },
  );

  const currentCatalog = createValueMemo((): CatalogLoad | null => {
    const load = catalog();
    if (load === undefined) {
      return null;
    }
    return load.worldId === worldFilter() &&
      load.page === catalogPage() &&
      load.pageSize === catalogPageSize()
      ? load
      : null;
  });
  const catalogData = createValueMemo((): AdminSettlementCatalogPage | null => {
    const load = currentCatalog();
    return load?.result.ok ? load.result.value : null;
  });
  const catalogLookupState = createValueMemo((): CatalogLookupLoad | null =>
    matchingLookup(catalogLookup() ?? null, worldFilter(), requestedSequence()),
  );
  const lookupNeeded = createValueMemo((): boolean => {
    const page = catalogData();
    if (page === null || requestedSequence() === null) {
      return false;
    }
    return (
      resolveCatalogSelection(page, null, selectedAuctionId(), requestedSequence()).kind ===
      "resolving"
    );
  });
  const lookupData = createValueMemo((): AdminSettlementCatalogPage | null => {
    const load = catalogLookupState();
    return load?.result.ok ? load.result.value : null;
  });
  const selection = createValueMemo(
    (): CatalogSelection =>
      resolveCatalogSelection(
        catalogData(),
        lookupData(),
        selectedAuctionId(),
        requestedSequence(),
      ),
  );
  const selectedEntry = createValueMemo((): AdminSettlementCatalogEntry | null => {
    const resolution = selection();
    return resolution.kind === "matched" ? resolution.entry : null;
  });
  const pickerCatalog = createValueMemo((): AdminSettlementCatalogPage | null => {
    const page = catalogData();
    if (page === null) {
      return null;
    }
    const entry = selectedEntry();
    if (
      entry !== null &&
      !page.auctions.some((candidate) => candidate.auctionId === entry.auctionId)
    ) {
      // 深链接目标不在当前分页时仍把它放入选择器；翻页后恢复该页的普通目录。
      return { ...page, auctions: [...page.auctions, entry] };
    }
    return { ...page, auctions: [...page.auctions] };
  });
  const catalogLoading = createValueMemo(
    () => currentCatalog() === null || (lookupNeeded() && catalogLookupState() === null),
  );
  const catalogError = createValueMemo((): string | null => {
    const current = currentCatalog();
    if (current !== null && !current.result.ok) {
      return adminApiErrorMessage(current.result.error);
    }
    if (lookupNeeded()) {
      const lookup = catalogLookupState();
      if (lookup !== null && !lookup.result.ok) {
        return adminApiErrorMessage(lookup.result.error);
      }
    }
    return selection().kind === "contradictory" ? adminApiErrorMessage("invalid_response") : null;
  });

  const currentSettlement = createValueMemo((): DetailLoad<AdminSettlementReport> | null => {
    const load = settlement();
    return load !== undefined && load.auctionId === selectedAuctionId() ? load : null;
  });
  // 恢复表单依赖恢复报告的版本；会话宿主收到回执后，旧报告不能授权新命令。
  const recoveryRead = createValueMemo(
    (): RecoveryRead => ({
      reload: reloadGenerationNumber,
      recoveryReload: recoveryReloadGeneration,
      generation: [reloadGenerationNumber(), recoveryReloadGeneration()],
      alive: () => alive,
    }),
  );
  const currentRecovery = createValueMemo((): DetailLoad<AdminRecoveryReport> | null => {
    const state = recovery();
    if (state === undefined) {
      return null;
    }
    const [generation, load] = state;
    const currentGeneration = recoveryRead().generation;
    return generation[0] === currentGeneration[0] &&
      generation[1] === currentGeneration[1] &&
      load.auctionId === selectedAuctionId()
      ? load
      : null;
  });
  const detail = createValueMemo((): SettlementDetail => {
    const entry = selectedEntry();
    const settlementLoad = currentSettlement();
    const recoveryLoad = currentRecovery();
    let detailError: string | null = null;
    if (entry !== null) {
      if (settlementLoad !== null && !settlementLoad.result.ok) {
        detailError = adminApiErrorMessage(settlementLoad.result.error);
      } else if (recoveryLoad !== null && !recoveryLoad.result.ok) {
        detailError = adminApiErrorMessage(recoveryLoad.result.error);
      }
    }
    const candidateReport = settlementLoad?.result.ok ? settlementLoad.result.value : null;
    const checked = reportForSelection(selection(), candidateReport);
    if (!checked.ok) {
      return { report: null, error: adminApiErrorMessage(checked.error) };
    }
    return { report: checked.value, error: detailError };
  });
  const recoveryReport = createValueMemo((): AdminRecoveryReport | null => {
    const report = detail().report;
    const load = currentRecovery();
    return report !== null && load?.result.ok ? load.result.value : null;
  });
  // 确认框和命令入口都必须由目录与详情的同一条场次事实授权。
  const recoveryDraft = createValueMemo((): RecoveryConfirmationDraft | null => {
    const draft = confirmation();
    const report = recoveryReport();
    if (draft === null || report === null) {
      return null;
    }
    return report.recoveries.some(
      (entry) => entry.id === draft.recoveryId && entry.state_version === draft.stateVersion,
    )
      ? draft
      : null;
  });
  const detailLoading = createValueMemo(
    () =>
      selectedEntry() !== null &&
      selectedAuctionId() !== "" &&
      (currentSettlement() === null || currentRecovery() === null),
  );
  const reportTitle = (): string => {
    const entry = selectedEntry();
    return entry === null
      ? t("admin-settlement-report-panel-title")
      : t("admin-settlement-report-title", {
          world: adminWorldLabel(entry.worldId),
          sequence: entry.sequence,
        });
  };

  const viewState: AdminSettlementViewState = {
    worldFilter,
    selectedAuctionId,
    requestedSequence,
    catalogPage,
    catalogPagination,
    lotPage,
    recoveryPage,
    confirmation,
    reason,
    actionFeedback,
    selectionGeneration,
    recoveryReloadGeneration,
    pickerCatalog,
    catalogLoading,
    catalogError,
    catalogData,
    reportTitle,
    detailLoading,
    detail,
    recoveryReport,
    actionBusy,
    recoveryRead,
    recoveryDraft,
    pending,
  };
  return <AdminSettlementPageView state={viewState} />;
}
