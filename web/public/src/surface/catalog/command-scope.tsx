// 页面级竞价弹窗与确认草稿；已发出的命令只归外壳。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { useConfirmCommand } from "@auction/web-shared/net/commands";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { Result } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import {
  createContext,
  createEffect,
  Match,
  onCleanup,
  Show,
  Switch,
  untrack,
  useContext,
} from "solid-js";

import type { OwnedProxy } from "../../api/account";
import type { CommandError } from "../../api/commands";
import type { CatalogRow } from "../../api/model";
import {
  PublicCommandOwnerContext,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "..";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { BidForm } from "../commands/bid";
import { CompetitionCommandsContext } from "../commands/host";
import {
  type PendingBidCommand,
  type PendingProxyCommand,
  PublicCommandIdentityContext,
} from "../commands/pending";
import { activeOwnedProxy, type ProxyFormInput, proxyForm } from "../commands/proxy";
import { type CommandPayload, type CommandReceipt, commandBusy } from "../commands/request";
import { CommandRecovery, commandReceiptMessage } from "../commands/retry";
import { queryAuctionId, readPublicQuery } from "../query";
import {
  currentUser,
  useWatchCommandOwner,
  WatchCommandOwnerContext,
  WatchCommandStatus,
} from "../watches/commands";
import type { CatalogDetailInput } from "./detail";
import { CatalogDetailDialogContext } from "./detail";
import { CatalogToastContext, CatalogToastHost, useCatalogToastProvider } from "./toast";

/** 目录命令弹窗的两种动作；切换只替换表单，不新建命令状态。 */
export type CatalogAction = "bid" | "proxy";

/** 这里只保存弹窗展示输入；已发送的负载与结果由外壳唯一管理。 */
export interface CommandDialogInput {
  readonly lotId: string;
  readonly lotCode: string;
  /** 装备名只用于弹窗摘要，不进入命令负载。 */
  readonly lotName: string;
  readonly action: CatalogAction;
  readonly canBid: boolean;
  readonly currentPrice: number;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式；往期目录拼装的已结算场次为 null。 */
  readonly proxyDefaultMode: string | null;
  readonly ordinaryBlocked: boolean;
  readonly leading: boolean;
  readonly blockedReason: string;
  readonly proxy: Result<OwnedProxy | null, string> | null;
  /** 本轮授权读取成功时的刷新代号，缺省禁止新的代理写入。 */
  readonly proxyReadGeneration: number | null;
  /** 当前读取失败时同时携带最后确认的授权，供普通出价判定与表单草稿使用。 */
  readonly proxyReadError?: string | null;
}

/** 页面级弹窗信号；目录外的只读入口没有提供方。 */
export const CommandDialogContext = createContext<Cell<CommandDialogInput | null> | null>(null);

/** 恢复入口可在拍品结束时挂载，输入不具备发出新命令的资格。 */
export function recoveryDialogInput(row: CatalogRow): CommandDialogInput {
  return {
    lotId: row.lotId,
    lotCode: row.code,
    lotName: row.name,
    action: "bid",
    canBid: false,
    currentPrice: row.currentPrice,
    nextBid: row.nextBid,
    lotStatus: row.status,
    allowSlow: false,
    proxyDefaultMode: null,
    ordinaryBlocked: true,
    leading: false,
    blockedReason: "",
    proxy: null,
    proxyReadGeneration: null,
  };
}

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 当前快照无新确认时，由外壳最后确认的宿主继续持有草稿。 */
export function catalogIdentityMatches(
  snapshot: PublicSessionResult | undefined,
  hostedUserId: string,
  confirmedUserId: string | null,
): boolean {
  const hosted = canonicalUuid(hostedUserId);
  return (
    hosted !== null &&
    hosted !== NIL_UUID &&
    canonicalUuid(confirmedUserId) === hosted &&
    (snapshot?.ok !== true || canonicalUuid(currentUser(snapshot)) === hosted)
  );
}

/** 新写入只允许经当前已就绪的同身份会话，避免把未复核的草稿提交到另一账号。 */
export function catalogCommandSession(
  snapshot: PublicSessionResult | undefined,
  ready: boolean,
  hostedUserId: string,
  confirmedUserId: string | null,
  targetAllowsBid = true,
): { authenticated: boolean; canSubmit: boolean } {
  const authenticated = catalogIdentityMatches(snapshot, hostedUserId, confirmedUserId);
  return {
    authenticated,
    canSubmit: targetAllowsBid && authenticated && ready && snapshot?.ok === true,
  };
}

/** 看板按已确认用户建键，仅保存本页的草稿、标记与抽屉。 */
export function CatalogCommandScope(props: {
  readonly userId: string | null;
  readonly auction: CurrentAuctionItemResponse | null;
  readonly auctionInvalid: boolean;
  /** 目标是否必须与地址栏的世界与场次一致；缺省一致（看板）。“我的”页的查看抽屉不在看板地址上，
   * 目标由抽屉自己的拍品固定，传 false 只比较弹窗打开时的场次。 */
  readonly followsPageQuery?: boolean;
  readonly children: JSX.Element;
}): JSX.Element {
  const dialog = createCell<CommandDialogInput | null>(null);
  // 详情抽屉的展示信号与命令弹窗同属身份宿主：身份切换时两者一起清空。抽屉本身不产生
  // 任何服务端命令，并由目录内的 CatalogDetailHost 渲染（它需要标记作用域）。
  const detail = createCell<CatalogDetailInput | null>(null);
  // 成功提示条比发出它的命令弹窗活得久，因此由身份宿主持有并常驻渲染。
  const toast = useCatalogToastProvider();
  const pendingWatches = useWatchCommandOwner(props.userId);
  const userId = canonicalUuid(props.userId) ?? NIL_UUID;
  const identity = { userId };

  return (
    <WatchCommandOwnerContext value={pendingWatches}>
      <CommandDialogContext value={dialog}>
        <CatalogDetailDialogContext value={detail}>
          <CatalogToastContext value={toast.context}>
            <PublicCommandIdentityContext value={identity}>
              {props.children}
              <WatchCommandStatus owner={pendingWatches} />
              <Show when={dialog()}>
                {(current) => (
                  <CatalogCommandDialog
                    input={current()}
                    dialog={dialog}
                    auction={props.auction}
                    auctionInvalid={props.auctionInvalid}
                    followsPageQuery={props.followsPageQuery !== false}
                  />
                )}
              </Show>
              <CatalogToastHost toast={toast.toast} />
            </PublicCommandIdentityContext>
          </CatalogToastContext>
        </CatalogDetailDialogContext>
      </CommandDialogContext>
    </WatchCommandOwnerContext>
  );
}

function CatalogCommandDialog(props: {
  readonly input: CommandDialogInput;
  readonly dialog: Cell<CommandDialogInput | null>;
  readonly auction: CurrentAuctionItemResponse | null;
  readonly auctionInvalid: boolean;
  readonly followsPageQuery: boolean;
}): JSX.Element {
  const input = () => props.input;
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const session = useContext(PublicSessionResourceContext);
  const owner = useContext(PublicCommandOwnerContext);
  const identity = useContext(PublicCommandIdentityContext);
  const competition = useContext(CompetitionCommandsContext);
  const store = () => competition?.current() ?? null;
  const origin = Symbol("command-dialog");
  let mounted = true;
  const openedAuction = props.auction;
  // popstate 已改地址时，不能用新场次或世界替换打开弹窗时的目标。
  const targetAllowsBid = () => {
    if (!props.followsPageQuery) {
      return (
        !props.auctionInvalid &&
        openedAuction !== null &&
        props.auction?.id === openedAuction.id &&
        props.auction.world === openedAuction.world
      );
    }
    const query = readPublicQuery();
    const selected = queryAuctionId(query);
    return (
      !props.auctionInvalid &&
      query.auction.kind !== "invalid" &&
      openedAuction !== null &&
      props.auction?.id === openedAuction.id &&
      props.auction.world === openedAuction.world &&
      (selected === null || selected === openedAuction.id) &&
      (query.world ?? "main") === openedAuction.world
    );
  };
  const commandSession = () =>
    catalogCommandSession(
      session(),
      session.state() === "ready",
      identity.userId,
      owner(),
      targetAllowsBid(),
    );
  const toasts = useContext(CatalogToastContext);
  const feedback = createCell<CommandReceipt | null>(null);
  const bidCommand = useConfirmCommand<PendingBidCommand, CommandError>();
  const proxyCreateCommand = useConfirmCommand<PendingProxyCommand, CommandError>();
  const proxyLowerCommand = useConfirmCommand<PendingProxyCommand, CommandError>();
  const pending = () => store()?.command(input().lotId) ?? null;
  const busy = () => {
    const current = pending();
    return current !== null && commandBusy(current);
  };
  const proxySaved = () => feedback()?.error === null;
  const canSubmit = () =>
    mounted && pending() === null && !proxySaved() && commandSession().canSubmit;
  const canSubmitProxy = () => canSubmit() && input().proxyReadGeneration === refreshGeneration();
  const confirming = () =>
    pending() === null &&
    (bidCommand.confirmation() !== null ||
      proxyCreateCommand.confirmation() !== null ||
      proxyLowerCommand.confirmation() !== null);
  createEffect(
    () => store()?.dialogReceipt(origin),
    (receipt) => {
      if (receipt == null) return;
      const taken = competition?.current.peek()?.takeDialogReceipt(origin);
      if (taken == null) return;
      if (taken.error === null && taken.kind === "bid") {
        toasts.show(
          taken.target.lotCode,
          untrack(() => commandReceiptMessage(taken)),
          null,
        );
        props.dialog.set(null);
      } else {
        feedback.set(taken);
      }
    },
  );
  onCleanup(() => {
    mounted = false;
    competition?.current.peek()?.detach(origin);
  });
  const submit = (payload: CommandPayload) => {
    const current = input();
    if (
      !mounted ||
      openedAuction === null ||
      !targetAllowsBid() ||
      !current.canBid ||
      (payload.kind !== "bid" && !canSubmitProxy()) ||
      props.dialog.peek()?.lotId !== current.lotId
    )
      return;
    if (
      store()?.submit(
        {
          world: openedAuction.world,
          auctionId: openedAuction.id,
          lotId: current.lotId,
          lotCode: current.lotCode,
          lotName: current.lotName,
        },
        payload,
        origin,
      )
    ) {
      // 外壳接管后清理未发送的副本；原样重试只能从外壳读取冻结负载。
      bidCommand.reset();
      proxyCreateCommand.reset();
      proxyLowerCommand.reset();
      feedback.set(null);
    }
  };
  const close = () => {
    if (busy()) {
      return;
    }
    if (confirming()) {
      // 确认态的关闭/Escape 先退回编辑态，而不是结束整条命令。
      bidCommand.cancelConfirmation();
      proxyCreateCommand.cancelConfirmation();
      proxyLowerCommand.cancelConfirmation();
      return;
    }
    // 显式关闭沿用原有取消契约；自动重读绝不走这个路径。
    props.dialog.set(null);
  };
  // 切换 tab 走同一弹窗快照：actions.rs 的代理资源与刷新订阅据此激活，不另建状态。
  const switchTab = (action: CatalogAction) => {
    const next = props.dialog.peek();
    if (next === null || next.action === action || pending() !== null || !targetAllowsBid()) {
      return;
    }
    props.dialog.set({ ...next, action });
  };
  const title = () =>
    confirming()
      ? t("bid-confirm-title")
      : pending() !== null || input().action === "bid"
        ? t("bid-dialog-title", { code: input().lotCode })
        : t("proxy-dialog-title", { code: input().lotCode });
  const canBid = () => targetAllowsBid() && input().canBid && commandSession().authenticated;
  const activeProxy = () => activeOwnedProxy(input().proxy) !== null;
  const proxyInput = (): ProxyFormInput => ({
    lotId: input().lotId,
    authenticated: commandSession().authenticated,
    canBid: canBid(),
    canSubmit: canSubmitProxy(),
    currentPrice: input().currentPrice,
    nextBid: input().nextBid,
    lotStatus: input().lotStatus,
    allowSlow: input().allowSlow,
    defaultMode: input().proxyDefaultMode,
    leading: input().leading,
    blockedReason: input().blockedReason,
    proxy: input().proxy,
    proxyReadError: input().proxyReadError ?? null,
    refreshGeneration,
    createCommand: proxyCreateCommand,
    lowerCommand: proxyLowerCommand,
    onSubmit: submit,
    onClose: close,
  });
  const currentPrice = () => formatCreditAmount(input().currentPrice);
  const nextBid = () =>
    input().nextBid > MAX_PUBLIC_BID_AMOUNT
      ? t("bid-limit-reached")
      : formatCreditAmount(input().nextBid);

  return (
    // 确认视图会把焦点放到“确认”按钮上：按住 Enter 提交金额时，浏览器的重复按键不能
    // 顺势落在确认按钮上替用户完成二次确认。包裹层不参与布局，只吞掉重复的 Enter。
    // biome-ignore lint/a11y/noStaticElementInteractions: 包裹层只吞掉重复 Enter，焦点管理在 Dialog。
    <div
      class="public-bid-dialog__keys"
      onKeyDown={(event) => {
        if (event.key === "Enter" && event.repeat) {
          event.preventDefault();
        }
      }}
    >
      {/* 焦点管理由 PublicShell 统一提供。 */}
      <Dialog title={title()} closeLabel={t("catalog-close")} busy={busy()} onClose={close}>
        <Show when={pending() === null}>
          <Show
            when={confirming()}
            fallback={
              <>
                {/* 拍品摘要块：编号、装备名与两个价格在表单之前一次可见。 */}
                <div class="public-bid-dialog__lot">
                  <div class="public-bid-dialog__lot-name">
                    <span class="public-bid-dialog__lot-code">{input().lotCode}</span>
                    <span class="public-bid-dialog__lot-title">{input().lotName}</span>
                  </div>
                  <div class="public-bid-dialog__lot-facts">
                    <div>
                      <span>{t("catalog-current")}</span>
                      <b>{currentPrice()}</b>
                    </div>
                    <div>
                      <span>{t("catalog-next-bid")}</span>
                      <b>{nextBid()}</b>
                    </div>
                  </div>
                </div>
                <Show when={!proxySaved() && !props.auctionInvalid}>
                  {/* 直接出价/代理出价共用同一弹窗；切换只替换表单内容，不新建命令状态。 */}
                  {/* biome-ignore lint/a11y/useSemanticElements: 源码使用 div role=group 保持 DOM 结构。 */}
                  <div
                    class="public-bid-dialog__tabs"
                    role="group"
                    aria-label={t("bid-type-label")}
                  >
                    <button
                      class={
                        input().action === "bid"
                          ? "public-bid-dialog__tab public-bid-dialog__tab--active"
                          : "public-bid-dialog__tab"
                      }
                      type="button"
                      aria-pressed={input().action === "bid" ? "true" : "false"}
                      disabled={busy() || activeProxy()}
                      // 仅在已有生效代理时用悬停说明禁用原因；不渲染常驻提示。
                      title={activeProxy() ? t("proxy-active-exists") : undefined}
                      onClick={() => {
                        if (!activeProxy()) {
                          switchTab("bid");
                        }
                      }}
                    >
                      {t("bid-tab-direct")}
                    </button>
                    <button
                      class={
                        input().action === "proxy"
                          ? "public-bid-dialog__tab public-bid-dialog__tab--active"
                          : "public-bid-dialog__tab"
                      }
                      type="button"
                      aria-pressed={input().action === "proxy" ? "true" : "false"}
                      disabled={busy()}
                      onClick={() => switchTab("proxy")}
                    >
                      {t("bid-tab-proxy")}
                    </button>
                  </div>
                </Show>
              </>
            }
          >
            {/* 确认态：紧凑拍品行代替摘要块与切换条；类型、金额与约束说明
          由当前表单的确认面板给出。 */}
            <div class="public-bid-confirm__lot">
              <span class="public-bid-confirm__lot-code">{input().lotCode}</span>
              <b class="public-bid-confirm__lot-title">{input().lotName}</b>
            </div>
          </Show>
        </Show>
        <div role="status" aria-atomic="true">
          <Show when={feedback()}>
            {(receipt) => (
              <p
                class={
                  proxySaved()
                    ? "action-feedback action-feedback--success"
                    : "action-feedback action-feedback--error"
                }
              >
                {commandReceiptMessage(receipt())}
              </p>
            )}
          </Show>
        </div>
        <Switch fallback={null}>
          <Match when={pending()}>
            {(command) => <CommandRecovery command={command()} origin={origin} />}
          </Match>
          <Match when={proxySaved()}>
            <div class="public-auction-bid-form__actions">
              <button type="button" autofocus={true} onClick={close}>
                {t("catalog-close")}
              </button>
            </div>
          </Match>
          <Match when={input().action === "bid"}>
            <BidForm
              input={{
                lotId: input().lotId,
                nextBid: input().nextBid,
                lotStatus: input().lotStatus,
                canBid: canBid(),
                canSubmit: canSubmit(),
                blocked: input().ordinaryBlocked,
                blockedReason: input().blockedReason,
                command: bidCommand,
                onSubmit: (payload) => submit({ ...payload, kind: "bid" }),
                onClose: close,
              }}
            />
          </Match>
          <Match when={input().action === "proxy"}>{proxyForm(proxyInput)}</Match>
        </Switch>
      </Dialog>
    </div>
  );
}

function canonicalUuid(value: string | null): string | null {
  if (value === null) {
    return null;
  }
  let hyphenated = value;
  if (value.length === 38 && value.startsWith("{") && value.endsWith("}")) {
    hyphenated = value.slice(1, -1);
  } else if (value.length === 45 && value.slice(0, 9).toLowerCase() === "urn:uuid:") {
    hyphenated = value.slice(9);
  } else if (value.length === 32 && /^[0-9a-fA-F]{32}$/u.test(value)) {
    hyphenated = `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`;
  }
  if (
    hyphenated.length !== 36 ||
    !/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/u.test(
      hyphenated,
    )
  ) {
    return null;
  }
  return hyphenated.toLowerCase();
}
