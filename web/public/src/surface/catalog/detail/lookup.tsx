// 目录之外按拍品 ID 打开的只读详情抽屉：账户页这类列表只有拍品 ID，没有目录行快照。
// 先读取拍品详情，就绪后才渲染抽屉，避免空外壳滑入后再换内容；抽屉不带标记入口。出价入口
// 只在宿主声明了账号（`userId`）时提供，资格与命令仍复用目录的判定与命令作用域（见 lookup-bid.tsx）。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import {
  createContext,
  createEffect,
  For,
  Match,
  Show,
  Switch,
  untrack,
  useContext,
} from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { catalogRowFromDetail } from "../../../api/model";
import { fetchLotDetail, type LotDetailFetchError } from "../../../api/queries/lots";
import { PublicRefreshGenerationContext, useBidRefresh } from "../../board/refresh";
import { CatalogActionIcon } from "../actions";
import type { CatalogDetailBid, DetailWorld } from ".";
import { catalogDetailBidDefault } from ".";
import { CatalogDetailDrawer } from "./drawer";
import { LookupBiddingDrawer } from "./lookup-bid";
import type { LotViewRequest, LotViewTarget } from "./lot-view";

/** 一次查看请求指向的拍品。`auctionId` 用于抽屉内的竞价历史读取；`world` 是入口已知的
 * 拍品所在世界，未知时价格历史按拍卖读取。仅供测试使用。 */
export interface LotDetailTarget {
  readonly auctionId: string;
  readonly lotId: string;
  readonly world: string | null;
  readonly lotCode: string;
  /** 仅从深链交付的请求绑定历史条目；行内查看仍随本页宿主持有。 */
  readonly entryKey: string | null;
}

type LotDetailSnapshot = Pick<LotViewTarget, "row" | "refreshGeneration">;

/** 查看状态；只有加载中或已展示时才渲染抽屉。仅供测试使用。 */
export type LookupState =
  | { readonly kind: "closed" }
  /** 首次读取中：抽屉尚未渲染，行内按钮显示忙碌。 */
  | {
      readonly kind: "loading";
      readonly target: LotDetailTarget;
      readonly prefetched?: LotDetailSnapshot;
    }
  | { readonly kind: "open"; readonly target: LotDetailTarget }
  /** 保留失败的目标与分类，暂时故障只重读原拍品。 */
  | {
      readonly kind: "failed";
      readonly target: LotDetailTarget;
      readonly error: LotDetailFetchError;
    };

function targetsEqual(left: LotDetailTarget, right: LotDetailTarget): boolean {
  return (
    left.auctionId === right.auctionId &&
    left.lotId === right.lotId &&
    left.world === right.world &&
    left.entryKey === right.entryKey
  );
}

function belongsToEntry(target: LotDetailTarget, entryKey: string): boolean {
  return target.entryKey === null || target.entryKey === entryKey;
}

/** 新的查看请求。同一拍品读取中或已展示时返回 `null`，重复点击不重启读取。
 * 仅供测试使用。 */
export function requested(state: LookupState, target: LotDetailTarget): LookupState | null {
  if ((state.kind === "loading" || state.kind === "open") && targetsEqual(state.target, target)) {
    return null;
  }
  return { kind: "loading", target };
}

/** 读取完成后的迁移。暂时性错误只允许保留同一读取代号的快照。
 * 已关闭或改看其他拍品的结果直接丢弃。仅供测试使用。 */
export function resolved(
  state: LookupState,
  target: LotDetailTarget,
  error: LotDetailFetchError | null,
  shownInGeneration: boolean,
): LookupState | null {
  if ((state.kind !== "loading" && state.kind !== "open") || !targetsEqual(state.target, target)) {
    return null;
  }
  if (error === null) {
    return state.kind === "loading" ? { kind: "open", target: state.target } : null;
  }
  return error.kind === "not_found" || !shownInGeneration
    ? { kind: "failed", target: state.target, error }
    : null;
}

/** 查找句柄私有状态的键；模块外不可见，调用方只能通过上下文取得句柄。 */
const LOOKUP_STATE = Symbol("lot-detail-lookup-state");

/** 查看请求的句柄。行组件只写状态、不持有读取任务：任务归宿主所有，行随分页卸载时
 * 读取不会被取消，忙碌态也不会卡住。 */
export interface LotDetailLookup {
  readonly [LOOKUP_STATE]: Cell<LookupState>;
  /** 看板无法交给目录行时，连同条目标识交付预读详情。 */
  view(request: LotViewRequest): void;
}

function lookupState(lookup: LotDetailLookup): Cell<LookupState> {
  return lookup[LOOKUP_STATE];
}

function openTarget(lookup: LotDetailLookup, target: LotDetailTarget): void {
  const state = lookupState(lookup);
  const next = requested(state.peek(), target);
  if (next !== null) {
    state.set(next);
  }
}

/** 建立查找状态句柄。调用方须把后代包在 `LotDetailLookupContext` 内，并把
 * `LotDetailLookupHost` 渲染在不随分页或重读卸载的位置。 */
export function useLotDetailLookup(): LotDetailLookup {
  const state = createCell<LookupState>({ kind: "closed" });
  const lookup: LotDetailLookup = {
    [LOOKUP_STATE]: state,
    view({ entryKey, target: { auctionId, lotId, world, row, refreshGeneration } }) {
      state.set({
        kind: "loading",
        target: { auctionId, lotId, world, lotCode: row.code, entryKey },
        prefetched: { row, refreshGeneration },
      });
    },
  };
  return lookup;
}

/** 查找句柄所在上下文；列表行通过它取得同一份状态。 */
export const LotDetailLookupContext = createContext<LotDetailLookup | null>(null);

/** 列表行的查看按钮：外观与目录操作栏的查看入口一致。读取期间只标记忙碌而不禁用，
 * 抽屉关闭后焦点才能回到这枚按钮；重复点击由状态迁移忽略。 */
export function LotDetailLookupButton(props: {
  readonly auctionId: string;
  readonly lotId: string;
  readonly lotCode: string;
  /** 列表已知拍品所在世界时传入；账户列表只有拍卖 UUID，留空。 */
  readonly world?: string | null;
}): JSX.Element {
  const contextual = useContext(LotDetailLookupContext);
  if (contextual === null) {
    throw new Error("LotDetailLookupButton 必须在 LotDetailLookupContext 内渲染");
  }
  const lookup = contextual;
  const busy = () => {
    const state = lookupState(lookup)();
    return state.kind === "loading" && state.target.lotId === props.lotId;
  };
  return (
    <button
      class="public-lot-view"
      type="button"
      aria-haspopup="dialog"
      aria-busy={busy() ? "true" : "false"}
      // 可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
      aria-label={t("catalog-view-lot", { code: props.lotCode })}
      title={t("catalog-view")}
      onClick={() =>
        openTarget(lookup, {
          auctionId: props.auctionId,
          lotId: props.lotId,
          world: props.world ?? null,
          lotCode: props.lotCode,
          entryKey: null,
        })
      }
    >
      <CatalogActionIcon kind="eye" />
    </button>
  );
}

/** 详情抽屉的宿主。切换拍品或离开深链条目时，卸载读取任务并丢弃展示。
 * `userId` 为已确认的账号时抽屉带出价与代理入口，缺省为只读。 */
export function LotDetailLookupHost(props: {
  readonly lookup: LotDetailLookup;
  readonly userId?: string | undefined;
}): JSX.Element {
  const state = lookupState(props.lookup);
  const currentEntry = useRouter().currentEntryKey;
  createEffect(currentEntry, (entryKey) => {
    const snapshot = state.peek();
    // 深链是一次性查看意图，离开条目后即使前进回来也不能恢复旧抽屉或错误。
    if (snapshot.kind !== "closed" && !belongsToEntry(snapshot.target, entryKey)) {
      state.set({ kind: "closed" });
    }
  });
  const target = () => {
    const snapshot = state();
    return (snapshot.kind === "loading" || snapshot.kind === "open") &&
      belongsToEntry(snapshot.target, currentEntry())
      ? snapshot.target
      : null;
  };
  const targets = () => {
    const current = target();
    return current === null ? [] : [current];
  };
  return (
    <>
      <LotDetailFailure lookup={props.lookup} />
      <For
        each={targets()}
        keyed={(current) =>
          `${current.auctionId}:${current.lotId}:${current.world}:${current.entryKey ?? ""}`
        }
      >
        {(current) => (
          <LotDetailLookupSession target={current()} lookup={props.lookup} userId={props.userId} />
        )}
      </For>
    </>
  );
}

/** 详情读取失败有独立目标，重试不重读所在页面。 */
function LotDetailFailure(props: { readonly lookup: LotDetailLookup }): JSX.Element {
  const state = lookupState(props.lookup);
  const currentEntry = useRouter().currentEntryKey;
  const failure = () => {
    const current = state();
    return current.kind === "failed" && belongsToEntry(current.target, currentEntry())
      ? current
      : null;
  };
  return (
    <Switch>
      <Match when={failure()?.error.kind === "not_found" ? failure() : null}>
        {(failed) => (
          <AsyncState
            kind="empty"
            title={t("catalog-view-lot", { code: failed().target.lotCode })}
            detail={t("catalog-detail-not-found")}
          />
        )}
      </Match>
      <Match when={failure()}>
        {(failed) => (
          <AsyncState
            kind="error"
            title={t("catalog-view-lot", { code: failed().target.lotCode })}
            detail={t("data-error")}
            retryLabel={t("data-retry")}
            onRetry={() => {
              const current = state.peek();
              if (
                current.kind === "failed" &&
                current.error.kind === "lot_detail_fetch_error" &&
                belongsToEntry(current.target, untrack(currentEntry)) &&
                current === failed()
              ) {
                openTarget(props.lookup, current.target);
              }
            }}
          />
        )}
      </Match>
    </Switch>
  );
}

/** 一件拍品从读取到关闭的生命周期。 */
function LotDetailLookupSession(props: {
  readonly target: LotDetailTarget;
  readonly lookup: LotDetailLookup;
  readonly userId?: string | undefined;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const state = lookupState(props.lookup);
  const currentEntry = useRouter().currentEntryKey;
  const shown = createCell<LotDetailSnapshot | null>(null);
  const detailDependencies = createValueMemo(() => ({
    target: props.target,
    // 权威刷新使用全局代号，普通出价只补读所属场次的详情。
    refreshGeneration: refreshGeneration(),
  }));
  const detail = createTaskResource(
    detailDependencies,
    async ({ target, refreshGeneration: generation }, context) => {
      if (!belongsToEntry(target, untrack(currentEntry))) return;
      const current = state.peek();
      const prefetched =
        current.kind === "loading" && targetsEqual(current.target, target)
          ? current.prefetched
          : undefined;
      // 目录等待期间竞价命令可能已推进代号。只有同代号的预读能直接作为首次展示。
      let row: CatalogRow | null =
        prefetched?.refreshGeneration === generation ? prefetched.row : null;
      let error: LotDetailFetchError | null = null;
      if (row === null) {
        const page = await fetchLotDetail(target.lotId, context.signal);
        if (!context.alive()) return;
        if (!page.ok) {
          error = page.error;
        } else if (page.value.lot === null || page.value.auction_id !== target.auctionId) {
          // 权威详情明确回答目标不存在或归属另一场次：重试不可能成功，按缺失结案。
          error = { kind: "not_found" };
        } else {
          row = catalogRowFromDetail(page.value.lot);
        }
      }
      // 只有当前条目与读取代号内的目标才能发布价格或失败状态。
      const latest = state.peek();
      if (
        !belongsToEntry(target, untrack(currentEntry)) ||
        refreshGeneration.peek() !== generation ||
        (latest.kind !== "loading" && latest.kind !== "open") ||
        !targetsEqual(latest.target, target)
      ) {
        return;
      }
      if (row !== null) {
        shown.set({ row, refreshGeneration: generation });
      }
      const next = resolved(latest, target, error, shown.peek()?.refreshGeneration === generation);
      if (next !== null) {
        state.set(next);
      }
    },
  );
  useBidRefresh(
    detail,
    () => true,
    () => props.target.auctionId,
  );
  const world = (): DetailWorld =>
    props.target.world === null
      ? { kind: "auction" }
      : { kind: "known", world: props.target.world };
  return (
    <Show when={shown()?.row}>
      {(row) => {
        const userId = props.userId;
        // 带账号的宿主整个生命周期都走同一棵树：拍品状态变化只改变出价入口，不重建抽屉。
        if (userId !== undefined) {
          return (
            <LookupBiddingDrawer
              userId={userId}
              auctionId={props.target.auctionId}
              row={row()}
              world={world()}
              onClose={() => state.set({ kind: "closed" })}
            />
          );
        }
        return (
          <CatalogDetailDrawer
            auctionId={props.target.auctionId}
            input={{
              row: row(),
              bid: catalogDetailBidDefault() satisfies CatalogDetailBid,
              watchable: false,
            }}
            world={world()}
            onClose={() => state.set({ kind: "closed" })}
          />
        );
      }}
    </Show>
  );
}
