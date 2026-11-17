// Public 竞价与代理竞价操作面板。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { ok, type Result } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, onCleanup, Show, Switch, useContext } from "solid-js";

import { fetchMyProxy, type OwnedProxy } from "../../api/account";
import { PublicRefreshGenerationContext, useBidRefresh } from "../board/refresh";
import { PublicCommandIdentityContext } from "../commands/pending";
import { loginHrefForCurrentLocation } from "../session";
import type { CatalogAction, CommandDialogInput } from "./command-scope";
import { CommandDialogContext } from "./command-scope";
import type { CatalogDetailBid, CatalogDetailBidSlot } from "./detail";
import { CatalogDetailDialogContext } from "./detail";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 目录行的出价与代理入口；行内按钮与详情抽屉共享同一份资格判定。 */
export function LiveCatalogActions(props: {
  readonly lotId: string;
  readonly lotCode: string;
  /** 只用于命令弹窗的拍品摘要，不进入任何命令负载。 */
  readonly lotName: string;
  readonly canBid: boolean;
  readonly currentPrice: number;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式，只决定代理新建表单的初始选项。 */
  readonly proxyDefaultMode: string | null;
  readonly sessionReady: boolean;
  readonly authenticated: boolean;
  readonly leading: boolean;
  /** 详情抽屉出价入口的投影。本组件是出价资格判定的唯一 owner：行内按钮与抽屉按钮读取
   * 同一份结果，卸载时失败关闭。 */
  readonly detailBid: CatalogDetailBidSlot;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const identity = useContext(PublicCommandIdentityContext);
  const dialog = useContext(CommandDialogContext);
  const droppedLot = props.lotId;
  const activeAction = createValueMemo((): CatalogAction | null => {
    const current = dialog?.();
    return current !== undefined && current !== null && current.lotId === props.lotId
      ? current.action
      : null;
  });
  // 详情抽屉打开本拍品时同样激活代理视图：抽屉要展示本人授权的上限与已用次数。memo 只在
  // “是否打开本拍品”变化时通知，抽屉快照随价格刷新不会重读。
  const detailDialog = useContext(CatalogDetailDialogContext);
  const detailOpen = createValueMemo(() => {
    const current = detailDialog();
    return current !== null && current.row.lotId === props.lotId;
  });
  const proxyViewActive = createValueMemo(
    () => props.authenticated && (activeAction() === "proxy" || detailOpen()),
  );
  const proxyDependencies = createValueMemo(() => {
    const active = activeAction() === "proxy" || detailOpen();
    // 目录里每行都挂载本资源。若常驻订阅全局 refresh_generation，一次刷新会调度
    // 与行数相同的资源任务（100 行 = 100 个任务）。闭合状态下资源恒为 Ok(None)，
    // 跟随刷新不产生任何可见变化；因此只在本行代理视图激活（代理弹窗或本拍品的
    // 详情抽屉打开）时才订阅刷新节拍，闭合时自动退订。API 没有批量代理查询端点，
    // 逐个预取会改变网络行为，故不做目录级一次性加载。
    const refresh = active ? refreshGeneration() : 0;
    return {
      lotId: props.lotId,
      userId: identity.userId,
      authenticated: props.authenticated,
      active,
      refresh,
    };
  });
  const proxyState = createTaskResource(proxyDependencies, async (dependencies, context) => {
    // fetched 区分“真实读取的授权”与闭合状态的占位 Ok(None)。
    const fetched =
      dependencies.authenticated && dependencies.userId !== NIL_UUID && dependencies.active;
    const result = fetched
      ? await fetchMyProxy(dependencies.userId, dependencies.lotId, context.signal)
      : ok(null);
    return { lotId: dependencies.lotId, fetched, result };
  });
  // enabled 用访问器传入：代理视图关闭后必须立即退订刷新节拍。
  useBidRefresh(proxyState, proxyViewActive);
  // 代理视图保持打开期间的刷新（任意拍品的竞价事件都会触发）沿用上一次真实读取的快照，
  // 新结果到达后整体替换：否则表单会被“加载中”顶替而卸载，丢掉已输入的上限、模式与焦点。
  const proxyReady = () => proxyState.state() === "ready";
  const proxySnapshot = (): Result<OwnedProxy | null, string> | null => {
    const snapshot = proxyState();
    if (snapshot === undefined || snapshot.lotId !== props.lotId) {
      return null;
    }
    if (
      !proxySnapshotUsable(proxyReady(), snapshot.fetched, proxyViewActive(), snapshot.result.ok)
    ) {
      return null;
    }
    return snapshot.result;
  };
  const activeProxy = (): OwnedProxy | null => {
    const snapshot = proxySnapshot();
    return snapshot?.ok && snapshot.value !== null && snapshot.value.status === "active"
      ? snapshot.value
      : null;
  };
  const proxyLeading = () => activeProxy()?.bid?.leading === true;
  const effectiveLeading = () => props.leading || proxyLeading();
  const limitReached = () => props.nextBid > MAX_PUBLIC_BID_AMOUNT;
  const ordinaryBlocked = () => limitReached() || effectiveLeading();
  // 无法新增出价时，仍允许持有者把已有授权降低到有效绑定价格。
  const proxyActionEnabled = () => {
    const proxy = activeProxy();
    if (proxy === null) {
      return !effectiveLeading() && !limitReached();
    }
    const lowerBound = proxy.capabilities.waiting ? props.nextBid : props.currentPrice;
    return proxy.capabilities.can_lower && lowerBound <= MAX_PUBLIC_BID_AMOUNT;
  };
  const actionReason = () => {
    if (!props.canBid) {
      return t("bid-unavailable");
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    if (activeProxy() !== null && !effectiveLeading()) {
      return t("proxy-active");
    }
    if (effectiveLeading()) {
      return t("bid-leading");
    }
    if (!proxyActionEnabled()) {
      return t("proxy-cannot-lower");
    }
    return "";
  };
  // 领先由装备高亮表达，行自身已带“正在领先”的描述，不再重复说明；超过金额上限等
  // 其余原因必须可见。不显示说明时按钮不引用它。
  const showActionReason = () => !(props.canBid && effectiveLeading() && !limitReached());
  const actionReasonId = () =>
    showActionReason() ? `public-auction-action-reason-${props.lotId}` : "";
  const input = createValueMemo(
    (): CommandDialogInput => ({
      lotId: props.lotId,
      lotCode: props.lotCode,
      lotName: props.lotName,
      action: "bid",
      canBid: props.canBid,
      currentPrice: props.currentPrice,
      nextBid: props.nextBid,
      lotStatus: props.lotStatus,
      allowSlow: props.allowSlow,
      proxyDefaultMode: props.proxyDefaultMode,
      ordinaryBlocked: ordinaryBlocked(),
      leading: effectiveLeading(),
      blockedReason: actionReason(),
      proxy: proxySnapshot(),
    }),
  );
  createEffect(
    () => input(),
    (next) => {
      if (dialog === null) {
        return;
      }
      const current = dialog.peek();
      if (current !== null && current.lotId === next.lotId) {
        const merged = { ...next, action: current.action };
        if (!structuralEquals(merged, current)) {
          dialog.set(merged);
        }
      }
    },
  );
  // 行内三个分支与抽屉入口共用同一组判定：已登录走命令弹窗，匿名且可出价走登录链接。
  const commandEntry = () => props.authenticated && props.canBid;
  const loginEntry = () => !commandEntry() && props.canBid && props.sessionReady && !limitReached();
  const bidDisabled = () => !props.canBid || ordinaryBlocked();
  const detailProjection = createValueMemo((): CatalogDetailBid => {
    if (commandEntry()) {
      const entryAction = detailEntryAction(bidDisabled(), props.canBid, effectiveLeading());
      const detailInput = {
        ...input(),
        action: entryAction ?? "bid",
      } satisfies CommandDialogInput;
      return {
        kind: "command",
        input: detailInput,
        disabled: entryAction === null,
      };
    }
    if (loginEntry()) {
      return { kind: "login" };
    }
    return { kind: "unavailable" };
  });
  const detailSlot = props.detailBid.cell;
  createEffect(
    () => detailProjection(),
    (next) => {
      if (!structuralEquals(detailSlot.peek(), next)) {
        detailSlot.set(next);
      }
    },
  );
  onCleanup(() => {
    // GET 错误、分页或阶段切换可卸载行，但不能卸载页面级弹窗或冻结命令。
    if (dialog !== null) {
      const current = dialog.peek();
      if (current !== null && current.lotId === droppedLot) {
        dialog.set({ ...current, canBid: false, proxy: null });
      }
    }
    // 拍品结束后行仍在，但操作栏已卸载：抽屉入口随之失败关闭，不保留最后一次资格。
    detailSlot.set({ kind: "unavailable" });
  });
  const bidLabel = () => t("catalog-bid-lot", { code: props.lotCode });
  const bidInput = () => input();

  return (
    <span class="public-auction-catalog__actions-list">
      {/* 目录行使用「查看 / 出价 / 标记」三个图标：查看入口属于行（任何阶段都有），
      这里只负责出价；代理在出价弹窗的页签与详情抽屉里。 */}
      <Switch>
        <Match when={commandEntry()}>
          <button
            class="public-auction-catalog__icon-action"
            type="button"
            aria-haspopup="dialog"
            aria-expanded={activeAction() === "bid" ? "true" : "false"}
            disabled={bidDisabled()}
            // 图标按钮的可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
            aria-label={bidLabel()}
            title={t("catalog-bid")}
            aria-describedby={bidDisabled() ? actionReasonId() : ""}
            onClick={() => {
              // 已打开的模态命令不接受另一个入口替换负载或目标。
              if (dialog !== null && dialog.peek() === null) {
                dialog.set({ ...bidInput(), action: "bid" });
              }
            }}
          >
            <CatalogActionIcon kind="gavel" />
          </button>
        </Match>
        <Match when={loginEntry()}>
          <Link
            class="public-auction-catalog__icon-action"
            href={loginHrefForCurrentLocation()}
            aria-label={t("catalog-login-to-bid")}
            title={t("catalog-login-to-bid")}
          >
            <CatalogActionIcon kind="gavel" />
          </Link>
        </Match>
      </Switch>
      <Show when={showActionReason() && (ordinaryBlocked() || !proxyActionEnabled())}>
        <span id={actionReasonId()} class="public-auction-catalog__leading-reason" role="note">
          {actionReason()}
        </span>
      </Show>
    </span>
  );
}

/** 详情抽屉命令入口打开的页签；null 表示入口禁用。
 *
 * 抽屉是目录里管理代理授权的唯一去处。普通出价可用时进
 * 直接出价（代理由弹窗页签切换）。领先者不能再出价，但可能持有需要降低的代理授权：目录行
 * 不预取授权，无法在这里判断，入口改进代理页签，由弹窗读取后给出降低表单或受阻说明。 */
function detailEntryAction(
  ordinaryDisabled: boolean,
  canBid: boolean,
  leading: boolean,
): CatalogAction | null {
  if (!ordinaryDisabled) {
    return "bid";
  }
  if (canBid && leading) {
    return "proxy";
  }
  return null;
}

/** 代理授权读取结果能否用于渲染。
 *
 * 读取完成（ready）的结果总是可用。刷新进行中只沿用上一次真实读取成功的结果，且代理
 * 视图必须仍然打开：闭合状态的占位值与读取失败的结果不沿用，首次读取完成前保持加载，
 * 不把未确认的“没有授权”当成事实，也不让旧错误盖住正在进行的重试。 */
function proxySnapshotUsable(
  ready: boolean,
  fetched: boolean,
  viewActive: boolean,
  succeeded: boolean,
): boolean {
  return ready || (fetched && viewActive && succeeded);
}

/** 目录行操作图标；1.5px stroke 与侧栏图标一致，只表达查看、出价语义。 */
export function CatalogActionIcon(props: { readonly kind: string }): JSX.Element {
  const path = () => {
    switch (props.kind) {
      case "eye":
        return "M1 12s4-7 11-7 11 7 11 7-4 7-11 7S1 12 1 12zM12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z";
      case "gavel":
        return "m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10M16 16l6-6M8 8l6-6M9 7l8 8M21 11l-8-8";
      default:
        return "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z";
    }
  };
  return (
    <svg
      class="public-auction-catalog__action-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d={path()} />
    </svg>
  );
}
