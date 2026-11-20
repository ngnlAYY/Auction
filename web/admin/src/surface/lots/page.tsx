// Admin lot 页面加载和错误状态。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, onCleanup, Show, Switch, untrack } from "solid-js";

import { type AdminLot, type AdminLotPage, fetchAdminLots } from "../../api/lots";
import { draftReadIsReady, useDraftPage } from "../auctions/editor";
import { useAdminSessionHost } from "../auth/shell";
import { useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { useLotsQueryState } from "../query/lots";
import type { SortDirection } from "../sort";
import { AdminLotEditDialog, type LotEdit, lotEditLotId } from "./edit";
import { lotUpdateNoticeLabel } from "./presentation";
import type { LotUpdateNotice } from "./state";
import { AdminLotTable } from "./table";

type LotsView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminLotPage };

/** 加载并展示一个拍卖的 Admin lot 安全投影。 */
export function page(auctionId: string): JSX.Element {
  // Router 会复用相同路由组件；迭代片段按拍卖身份重挂载资源与交互状态，历史跳转不能沿用旧场次。
  return (
    <For each={[auctionId]} keyed={(value) => value}>
      {(value) => <AdminAuctionLotsPage auctionId={value()} />}
    </For>
  );
}

function AdminAuctionLotsPage(props: { readonly auctionId: string }): JSX.Element {
  const host = useAdminSessionHost();
  // 分页与当前页排序跟地址栏双向同步（?page=&sort=&dir=），刷新与浏览器前进/后退都能恢复。
  const query = useLotsQueryState();
  const queryState = query.state;
  const requestedPage = createValueMemo(() => queryState().page);
  const reloadGeneration = createCommandReload(0n);
  let intakeEntity: string | null = null;
  const deferredIntakeEntities = new Set<string>();
  const pagination = useAdminPagination("lots", ["page_size"], 100, 100_000);
  const pageSize = pagination.sizeSignal();
  const draft = createCell<LotEdit | null>(null);
  const notice = createCell<LotUpdateNotice | null>(null);
  let alive = true;
  onCleanup(() => {
    alive = false;
    deferredIntakeEntities.clear();
  });
  const pageAlive = () => alive;
  const editedLot = createValueMemo(() => {
    const current = draft();
    return current === null ? null : lotEditLotId(current);
  });
  const editing = createValueMemo(() => editedLot() !== null);
  const [read, lots] = useDraftPage(
    pagination,
    reloadGeneration,
    editing,
    () => [props.auctionId, requestedPage()] as const,
    async (request: readonly [string, bigint], size: number, signal: AbortSignal) => {
      const page = request[1];
      if (page > BigInt(Number.MAX_SAFE_INTEGER)) {
        return err("admin_pagination_bounds");
      }
      const generation = reloadGeneration.peek();
      const result = await fetchAdminLots(host, request[0], Number(page), size, signal);
      if (
        alive &&
        !signal.aborted &&
        result.ok &&
        queryState.peek().page === page &&
        pageSize.peek() === size &&
        reloadGeneration.peek() === generation
      ) {
        intakeEntity = `intake:${result.value.auction.world_id}`;
        const shouldReload = deferredIntakeEntities.has(intakeEntity);
        deferredIntakeEntities.clear();
        if (shouldReload) {
          // 交付快照前推进世代，确保录入前的快照不能用于编辑。
          advanceCommandReload(reloadGeneration);
        }
      }
      return result;
    },
  );
  const visibleLots = createValueMemo(() => {
    const current = lots();
    if (current === null || !current.ok) {
      return null;
    }
    return current.value;
  });
  const pending = usePendingCommands(reloadGeneration, (entity) => {
    if (!alive) {
      return false;
    }
    if (entity === `auction:${untrack(() => props.auctionId)}`) {
      return true;
    }
    if (entity.startsWith("intake:")) {
      if (intakeEntity !== null) {
        return entity === intakeEntity;
      }
      // 世界由当前有效读取确认，先保留这次失效通知以便补读。
      deferredIntakeEntities.add(entity);
      return false;
    }
    const lotId = entity.startsWith("lot:") ? entity.slice("lot:".length) : null;
    if (lotId === null) {
      return false;
    }
    const current = untrack(visibleLots);
    return current === null || current.lots.some((lot: AdminLot) => lot.id === lotId);
  });
  const latestVersion = createValueMemo(() => {
    const lotId = editedLot();
    if (lotId === null) {
      return null;
    }
    const current = visibleLots();
    if (current === null) {
      return null;
    }
    const found = current.lots.find((lot: AdminLot) => lot.id === lotId);
    return found === undefined ? null : found.state_version;
  });
  const pendingInDialog = createValueMemo(() => {
    const lotId = editedLot();
    if (lotId === null) {
      return false;
    }
    const entity = `lot:${lotId}`;
    return pending.contains(entity) && !pending.isBusy(entity);
  });
  const lotsView = createValueMemo((): LotsView => {
    const current = lots();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", code: current.error };
    }
    return { kind: "ready", page: current.value };
  });
  const readyPage = createValueMemo(() => {
    const current = lotsView();
    return current.kind === "ready" ? current.page : null;
  });
  const loadError = createValueMemo(() => {
    const current = lotsView();
    return current.kind === "error" ? current : null;
  });

  return (
    <PendingCommandsContext value={pending}>
      <Show when={!pendingInDialog()}>
        <PendingCommandPanel owner={pending} />
      </Show>
      <div role="status">
        <Show when={notice()}>
          {(receipt) => (
            <p class="action-feedback action-feedback--success">
              {lotUpdateNoticeLabel(receipt())}
            </p>
          )}
        </Show>
      </div>
      <Switch
        fallback={
          <Show when={readyPage()}>
            {(page) => (
              <AdminLotTable
                page={page()}
                pagination={pagination}
                editsReady={draftReadIsReady(read())}
                sort={queryState().sort}
                direction={queryState().direction}
                onPageChange={(nextPage: number | bigint) => {
                  const next = {
                    ...queryState(),
                    page: typeof nextPage === "bigint" ? nextPage : BigInt(nextPage),
                  };
                  query.push(next);
                }}
                onSortChange={([sort, direction]: readonly [string, SortDirection]) => {
                  const next = { ...queryState(), sort, direction };
                  query.push(next);
                }}
                reloadGeneration={reloadGeneration}
                draft={draft}
                notice={notice}
              />
            )}
          </Show>
        }
      >
        <Match when={lotsView().kind === "loading"}>
          <AdminLotLoading />
        </Match>
        <Match when={loadError()}>
          {(error) => (
            <AdminLotLoadError
              detail={adminApiErrorMessage(error().code)}
              reloadGeneration={reloadGeneration}
            />
          )}
        </Match>
      </Switch>
      <Show when={editing()}>
        <AdminLotEditDialog
          draft={draft}
          latestVersion={latestVersion()}
          read={read()}
          reloadGeneration={reloadGeneration}
          notice={notice}
          host={host}
          pageAlive={pageAlive}
        />
      </Show>
    </PendingCommandsContext>
  );
}

function AdminLotLoading(): JSX.Element {
  return (
    <section class="admin-auction-page" aria-labelledby="auction-title">
      <h1 id="auction-title">{t("page-auction-lots-title", { auction: "…" })}</h1>
      <p class="admin-auction-intro">
        <Link href="/auctions">{t("admin-lots-back")}</Link>
      </p>
      <section>
        <AsyncState kind="loading" title={t("admin-lots-loading")} detail="" />
      </section>
    </section>
  );
}

function AdminLotLoadError(props: {
  readonly detail: string;
  readonly reloadGeneration: Cell<bigint>;
}): JSX.Element {
  return (
    <section class="admin-auction-page" aria-labelledby="auction-title">
      <h1 id="auction-title">{t("page-auction-lots-title", { auction: "…" })}</h1>
      <p class="admin-auction-intro">
        <Link href="/auctions">{t("admin-lots-back")}</Link>
      </p>
      <section>
        <AsyncState
          kind="error"
          title={props.detail}
          detail=""
          retryLabel={t("admin-lots-retry")}
          onRetry={() => advanceCommandReload(props.reloadGeneration)}
        />
      </section>
    </section>
  );
}
