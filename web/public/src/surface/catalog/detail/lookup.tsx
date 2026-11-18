// 目录之外按拍品 ID 打开的只读详情抽屉：账户页这类列表只有拍品 ID，没有目录行快照。
// 先读取拍品详情，就绪后才渲染抽屉，避免空外壳滑入后再换内容；抽屉不带出价与标记入口，
// 出价资格仍只由目录操作栏在拍卖作用域内判定。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, For, Match, Show, Switch, useContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { catalogRowFromDetail } from "../../../api/model";
import {
  fetchLotDetail,
  LOT_DETAIL_FETCH_ERROR,
  type LotDetailFetchError,
} from "../../../api/queries/lots";
import { PublicRefreshGenerationContext, useBidRefresh } from "../../board/refresh";
import { CatalogActionIcon } from "../actions";
import type { CatalogDetailBid, DetailWorld } from ".";
import { catalogDetailBidDefault } from ".";
import type { LotViewTarget } from "./deep-link";
import { CatalogDetailDrawer } from "./drawer";

/** 一次查看请求指向的拍品。`auctionId` 用于抽屉内的竞价历史读取；`world` 是入口已知的
 * 拍品所在世界，未知时价格历史按拍卖读取。仅供测试使用。 */
export interface LotDetailTarget {
  readonly auctionId: string;
  readonly lotId: string;
  readonly world: string | null;
}

/** 查看状态；只有加载中或已展示时才渲染抽屉。仅供测试使用。 */
export type LookupState =
  | { readonly kind: "closed" }
  /** 首次读取中：抽屉尚未渲染，行内按钮显示忙碌。 */
  | {
      readonly kind: "loading";
      readonly target: LotDetailTarget;
      readonly prefetched?: Pick<LotViewTarget, "row" | "refreshGeneration">;
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
    left.auctionId === right.auctionId && left.lotId === right.lotId && left.world === right.world
  );
}

/** 新的查看请求。同一拍品读取中或已展示时返回 `null`，重复点击不重启读取。
 * 仅供测试使用。 */
export function requested(state: LookupState, target: LotDetailTarget): LookupState | null {
  if ((state.kind === "loading" || state.kind === "open") && targetsEqual(state.target, target)) {
    return null;
  }
  return { kind: "loading", target };
}

/** 读取完成后的迁移。结果只对仍在等待的同一拍品生效：已展示时的后台重读失败保留
 * 上一次快照，已关闭或已改看其他拍品的迟到结果直接丢弃。仅供测试使用。 */
export function resolved(
  state: LookupState,
  lotId: string,
  error: LotDetailFetchError | null,
): LookupState | null {
  if (state.kind !== "loading" || state.target.lotId !== lotId) {
    return null;
  }
  return error === null
    ? { kind: "open", target: state.target }
    : { kind: "failed", target: state.target, error };
}

/** 查找句柄私有状态的键；模块外不可见，调用方只能通过上下文取得句柄。 */
const LOOKUP_STATE = Symbol("lot-detail-lookup-state");

/** 查看请求的句柄。行组件只写状态、不持有读取任务：任务归宿主所有，行随分页卸载时
 * 读取不会被取消，忙碌态也不会卡住。 */
export interface LotDetailLookup {
  readonly [LOOKUP_STATE]: Cell<LookupState>;
  /** 看板无法交给目录行时，交付解析页的详情及其读取代号。 */
  view(target: LotViewTarget): void;
  /** 最近一次查看请求是否读取失败。 */
  failed(): boolean;
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
    view({ auctionId, lotId, world, row, refreshGeneration }) {
      state.set({
        kind: "loading",
        target: { auctionId, lotId, world },
        prefetched: { row, refreshGeneration },
      });
    },
    failed() {
      return state().kind === "failed";
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
        })
      }
    >
      <CatalogActionIcon kind="eye" />
    </button>
  );
}

/** 只读详情抽屉的宿主。以拍品为 key：改看另一件时整体重建，上一件的读取随之取消。 */
export function LotDetailLookupHost(props: { readonly lookup: LotDetailLookup }): JSX.Element {
  const target = () => {
    const state = lookupState(props.lookup)();
    return state.kind === "loading" || state.kind === "open" ? state.target : null;
  };
  const targets = () => {
    const current = target();
    return current === null ? [] : [current];
  };
  return (
    <For each={targets()} keyed={(current) => current.lotId}>
      {(current) => <LotDetailLookupSession target={current()} lookup={props.lookup} />}
    </For>
  );
}

/** 看板交付到只读宿主后，拍品读取失败由此呈现，不与目录的重试合并。 */
export function BoardLotDetailFailure(props: { readonly lookup: LotDetailLookup }): JSX.Element {
  const state = lookupState(props.lookup);
  const failure = () => {
    const current = state();
    return current.kind === "failed" ? current : null;
  };
  return (
    <Switch>
      <Match when={failure()?.error.kind === "not_found"}>
        <AsyncState
          kind="empty"
          title={t("catalog-detail")}
          detail={t("catalog-detail-not-found")}
        />
      </Match>
      <Match when={failure()}>
        {(failed) => (
          <AsyncState
            kind="error"
            title={t("catalog-detail")}
            detail={t("data-error")}
            retryLabel={t("data-retry")}
            onRetry={() => {
              const current = state.peek();
              if (current === failed() && current.error.kind === "lot_detail_fetch_error") {
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
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const state = lookupState(props.lookup);
  const shown = createCell<CatalogRow | null>(null);
  const detailDependencies = createValueMemo(() => ({
    lotId: props.target.lotId,
    // 本人命令成功后推进的刷新代数；他人出价由下方的竞价通知补读。
    refreshGeneration: refreshGeneration(),
  }));
  const detail = createTaskResource(detailDependencies, async (dependencies, context) => {
    const current = state.peek();
    const prefetched =
      current.kind === "loading" && current.target.lotId === dependencies.lotId
        ? current.prefetched
        : undefined;
    // 目录等待期间竞价命令可能已推进代号。只有同代号的预读能直接作为首次展示。
    let row =
      prefetched?.refreshGeneration === dependencies.refreshGeneration ? prefetched.row : null;
    let error: LotDetailFetchError | null = null;
    if (row === null) {
      const page = await fetchLotDetail(dependencies.lotId, context.signal);
      if (!context.alive()) return;
      if (page.ok && page.value.lot !== null) {
        row = catalogRowFromDetail(page.value.lot);
      } else {
        error = page.ok ? LOT_DETAIL_FETCH_ERROR : page.error;
      }
    }
    if (row !== null) {
      shown.set(row);
    }
    // 读取任务处于响应式上下文内，这里只窥视状态，避免把迁移结果订阅成重读依赖。
    const next = resolved(state.peek(), dependencies.lotId, error);
    if (next !== null) {
      state.set(next);
    }
  });
  useBidRefresh(detail, () => true);
  const world = (): DetailWorld =>
    props.target.world === null
      ? { kind: "auction" }
      : { kind: "known", world: props.target.world };
  return (
    <Show when={shown()}>
      {(row) => {
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
