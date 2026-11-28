// Public 竞价与代理竞价操作面板。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import {
  createComponent,
  createContext,
  createEffect,
  Match,
  onCleanup,
  Show,
  Switch,
  useContext,
} from "solid-js";

import { fetchMyProxy, type OwnedProxy } from "../../api/account";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "..";
import { PublicRefreshGenerationContext, useBidRefresh } from "../board/refresh";
import { CompetitionCommandsContext } from "../commands/host";
import { PublicCommandIdentityContext } from "../commands/pending";
import { activeOwnedProxy } from "../commands/proxy";
import { commandEntryMessage } from "../commands/request";
import { loginHrefForCurrentLocation } from "../session";
import type { CatalogAction, CommandDialogInput } from "./command-scope";
import { CommandDialogContext, catalogIdentityMatches } from "./command-scope";
import type { CatalogDetailBid, CatalogDetailBidSlot } from "./detail";
import { CatalogDetailDialogContext } from "./detail";
import { CatalogProxySnapshotsContext, type ProxyRead } from "./proxy-snapshots";

const CatalogProxyAuctionContext = createContext<() => string>();

/** 目录行只访问实际展示场次的代理快照。 */
export function CatalogProxyScope(props: {
  readonly auctionId: string;
  readonly children: JSX.Element;
}): JSX.Element {
  return createComponent(CatalogProxyAuctionContext, {
    value: () => props.auctionId,
    get children() {
      return props.children;
    },
  });
}

/** 已结束的拍品也能打开未结命令，入口从不启用新的写入。 */
export function CatalogRecoveryEntry(props: { readonly input: CommandDialogInput }): JSX.Element {
  const competition = useContext(CompetitionCommandsContext);
  const dialog = useContext(CommandDialogContext);
  const pending = () => competition?.current()?.command(props.input.lotId) ?? null;
  const reasonId = () => `public-auction-command-recovery-${props.input.lotId}`;
  const label = () => t("command-recovery-open", { code: props.input.lotCode });
  return (
    <Show when={dialog !== null && pending()}>
      {(command) => (
        <span class="public-auction-catalog__actions-list">
          <button
            class="public-auction-catalog__icon-action"
            type="button"
            aria-haspopup="dialog"
            aria-describedby={reasonId()}
            aria-label={label()}
            title={label()}
            onClick={() => {
              if (dialog?.peek() === null && pending() !== null) {
                dialog.set({ ...props.input, canBid: false });
              }
            }}
          >
            <CatalogActionIcon kind="gavel" />
          </button>
          <span id={reasonId()} class="public-auction-catalog__leading-reason" role="note">
            {commandEntryMessage(command())}
          </span>
        </span>
      )}
    </Show>
  );
}

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
  /** 只维护出价资格与抽屉投影、不渲染操作栏：没有目录行的入口（“我的”页的查看抽屉）使用。 */
  readonly headless?: boolean;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const identity = useContext(PublicCommandIdentityContext);
  const confirmedOwner = useContext(PublicCommandOwnerContext);
  const session = useContext(PublicSessionResourceContext);
  const dialog = useContext(CommandDialogContext);
  const competition = useContext(CompetitionCommandsContext);
  const pending = () => competition?.current()?.command(props.lotId) ?? null;
  const hasPending = () => pending() !== null;
  const snapshots = useContext(CatalogProxySnapshotsContext);
  const auctionId = useContext(CatalogProxyAuctionContext);
  const droppedLot = props.lotId;
  // 弹窗可能由行按钮、抽屉或恢复入口打开：只按目标拍品判定归属，不跟踪对象身份。
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
  const identityConfirmed = createValueMemo(() =>
    catalogIdentityMatches(session(), identity.userId, confirmedOwner()),
  );
  const proxyViewActive = createValueMemo(
    () => (activeAction() === "proxy" || detailOpen()) && identityConfirmed(),
  );
  const proxyDependencies = createValueMemo(() => {
    // 只在本拍品的详情或代理页签打开时跟随刷新代号，避免闭合行随刷新调度空读取。
    return proxyViewActive()
      ? { auctionId: auctionId(), lotId: props.lotId, refresh: refreshGeneration() }
      : null;
  });
  const proxyState = createTaskResource(proxyDependencies, async (dependencies, context) => {
    if (dependencies === null) {
      return null;
    }
    const { auctionId: requestedAuction, lotId } = dependencies;
    const read = await fetchMyProxy(identity.userId, lotId, context.signal);
    if (!context.alive()) return null;
    const result =
      read.ok && read.value !== null && read.value.auction_id !== requestedAuction
        ? err("invalid_response")
        : read;
    return { request: dependencies, result };
  });
  // enabled 用访问器传入：代理视图关闭后必须立即退订刷新节拍。
  useBidRefresh(proxyState, proxyViewActive, auctionId);
  const scopedProxyRead = () => {
    const read = proxyState();
    return read != null &&
      read.request.auctionId === auctionId() &&
      read.request.lotId === props.lotId &&
      identityConfirmed()
      ? read
      : null;
  };
  createEffect(scopedProxyRead, (read) => {
    // 成功的同身份读取才能替换授权，包括明确无授权。读取失败不能解除活动授权对普通出价的限制。
    if (read?.result.ok === true) {
      snapshots.remember(identity.userId, read.request.auctionId, read.request.lotId, read.result);
    }
  });
  const currentProxyRead = createValueMemo(() =>
    usableProxySnapshot(
      scopedProxyRead()?.result ?? null,
      identityConfirmed(),
      proxyState.state() === "ready",
    ),
  );
  const proxyReadError = () => {
    const current = currentProxyRead();
    return current?.ok === false ? current.error : null;
  };
  const proxySnapshot = createValueMemo(() => {
    if (!identityConfirmed()) return null;
    const current = currentProxyRead();
    return current?.ok === true ? current : (snapshots.read(auctionId(), props.lotId) ?? current);
  });
  const proxyReadGeneration = () => {
    const read = scopedProxyRead();
    // 旧快照仍约束普通出价。代理写入须等当前视图这一次授权读取成功。
    return read?.result.ok === true &&
      read.request === proxyDependencies() &&
      proxyState.state() === "ready"
      ? read.request.refresh
      : null;
  };
  const activeProxy = () => activeOwnedProxy(proxySnapshot());
  const proxyLeading = () => activeProxy()?.bid?.leading === true;
  const effectiveLeading = () => props.leading || proxyLeading();
  const limitReached = () => props.nextBid > MAX_PUBLIC_BID_AMOUNT;
  const ordinaryBlocked = () =>
    ordinaryBidBlocked(effectiveLeading(), limitReached(), activeProxy());
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
    const command = pending();
    if (command !== null) {
      return commandEntryMessage(command);
    }
    if (!props.canBid) {
      return t("bid-unavailable");
    }
    // 已有生效代理时不显示常驻说明，也不落到下面其他原因。
    if (activeProxy() !== null) {
      return "";
    }
    if (limitReached()) {
      return t("bid-limit-note");
    }
    if (effectiveLeading()) {
      return t("bid-leading");
    }
    if (!proxyActionEnabled()) {
      return t("proxy-cannot-lower");
    }
    return "";
  };
  // 领先由装备高亮表达，行自身已带“正在领先”的描述；已有生效代理时也不说明。
  // 超过金额上限等其余原因必须可见。不显示说明时按钮不引用它。
  const showActionReason = () =>
    hasPending() ||
    !(props.canBid && (activeProxy() !== null || (effectiveLeading() && !limitReached())));
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
      proxyReadGeneration: proxyReadGeneration(),
      proxyReadError: proxyReadError(),
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
  const commandEntry = () => hasPending() || (props.authenticated && props.canBid);
  const loginEntry = () => !commandEntry() && props.canBid && props.sessionReady && !limitReached();
  const bidDisabled = () => !hasPending() && (!props.canBid || ordinaryBlocked());
  const detailProjection = createValueMemo((): CatalogDetailBid => {
    if (commandEntry()) {
      const entryAction = hasPending()
        ? "bid"
        : detailEntryAction(
            bidDisabled(),
            props.canBid,
            effectiveLeading(),
            activeProxy() !== null,
          );
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
        dialog.set({
          ...current,
          canBid: false,
          proxy: null,
          proxyReadGeneration: null,
          proxyReadError: null,
        });
      }
    }
    // 拍品结束后行仍在，但操作栏已卸载：抽屉入口随之失败关闭，不保留最后一次资格。
    detailSlot.set({ kind: "unavailable" });
  });
  const bidLabel = () => t("catalog-bid-lot", { code: props.lotCode });
  if (props.headless === true) {
    return null;
  }
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
            aria-describedby={bidDisabled() || hasPending() ? actionReasonId() : ""}
            onClick={() => {
              // 已打开的模态命令不接受另一个入口替换负载或目标。
              if (!bidDisabled() && dialog !== null && dialog.peek() === null) {
                // 恢复视图只看外壳记录。放弃后直接沿用本行的最新资格。
                dialog.set(input());
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
      <Show
        when={showActionReason() && (hasPending() || ordinaryBlocked() || !proxyActionEnabled())}
      >
        <span id={actionReasonId()} class="public-auction-catalog__leading-reason" role="note">
          {actionReason()}
        </span>
      </Show>
    </span>
  );
}

/** 已确认的本人活动授权期间，普通出价由代理管理入口接替。 */
export function ordinaryBidBlocked(
  leading: boolean,
  limitReached: boolean,
  activeProxy: OwnedProxy | null,
): boolean {
  return leading || limitReached || activeProxy !== null;
}

/** 详情抽屉命令入口打开的页签；null 表示入口禁用。
 *
 * 普通出价可用时进直接出价。已知有活动授权的玩家进入代理管理；领先者也可以进入代理页签，
 * 由弹窗读取后给出降低表单或受阻说明。 */
export function detailEntryAction(
  ordinaryDisabled: boolean,
  canBid: boolean,
  leading: boolean,
  hasActiveProxy = false,
): CatalogAction | null {
  if (!ordinaryDisabled) {
    return "bid";
  }
  if (canBid && (leading || hasActiveProxy)) {
    return "proxy";
  }
  return null;
}

/** 代理入口可用时，抽屉仍显示本拍品已禁止普通出价的原因。 */
export function detailBidReason(bid: CatalogDetailBid): string {
  if (bid.kind !== "command") {
    return "";
  }
  return bid.disabled || (bid.input.ordinaryBlocked && activeOwnedProxy(bid.input.proxy) !== null)
    ? bid.input.blockedReason
    : "";
}

/** 同身份沿用已读取的结果，重试期间暂时收起上一次读取错误。 */
export function usableProxySnapshot(
  snapshot: ProxyRead | null,
  identityConfirmed: boolean,
  ready: boolean,
): ProxyRead | null {
  return identityConfirmed && snapshot !== null && (ready || snapshot.ok) ? snapshot : null;
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
