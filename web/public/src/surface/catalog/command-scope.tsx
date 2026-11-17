// 页面级竞价弹窗宿主；读取分支与目录阶段只更新能力，不拥有已提交命令。

import { MAX_PUBLIC_BID_AMOUNT } from "@auction/contracts/dto/public/commands";
import { useConfirmCommand } from "@auction/web-shared/net/commands";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { Result } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, For, Match, Show, Switch, useContext } from "solid-js";

import type { OwnedProxy } from "../../api/account";
import type { CommandError, ProxyCommandAuthorization } from "../../api/commands";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { BidForm, type BidSuccess } from "../commands/bid";
import {
  type PendingBidCommand,
  type PendingProxyCommand,
  PublicCommandIdentityContext,
} from "../commands/pending";
import { type ProxyFormInput, proxyForm } from "../commands/proxy";
import { BidRetry } from "../commands/retry";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { useWatchCommandOwner } from "../watches/commands";
import type { CatalogDetailInput } from "./detail";
import { CatalogDetailDialogContext } from "./detail";
import { CatalogToastContext, CatalogToastHost, useCatalogToastProvider } from "./toast";

/** 目录命令弹窗的两种动作；切换只替换表单，不新建命令状态。 */
export type CatalogAction = "bid" | "proxy";

/** 这里只保存弹窗展示输入；UUID、冻结负载和未知结果仍由共享 ConfirmCommand 唯一管理。 */
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
}

/** 页面级弹窗信号；只读入口不在身份宿主之下时为 null，调用方按“没有提供方”处理。 */
export const CommandDialogContext = createContext<Cell<CommandDialogInput | null> | null>(null);

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 身份由外壳明确确认；暂时 GET 错误或 Pending 不改变宿主身份。 */
export function CatalogCommandScope(props: { readonly children: JSX.Element }): JSX.Element {
  const owner = useContext(PublicCommandOwnerContext);
  const userId = () => owner();
  // 单元素 keyed 列表建立真正的身份生命周期；静态子组件的 key 不会替换 scope。
  return (
    <For each={[userId()]} keyed={(value) => value}>
      {(current) => (
        <IdentityCommandScope userId={current()}>{props.children}</IdentityCommandScope>
      )}
    </For>
  );
}

function IdentityCommandScope(props: {
  readonly userId: string | null;
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
  const input = () => dialog();

  return (
    <CommandDialogContext value={dialog}>
      <CatalogDetailDialogContext value={detail}>
        <CatalogToastContext value={toast.context}>
          <PublicCommandIdentityContext value={identity}>
            {props.children}
            {pendingWatches}
            <Show when={input()}>
              {(current) => <CatalogCommandDialog input={current()} dialog={dialog} />}
            </Show>
            <CatalogToastHost toast={toast.toast} />
          </PublicCommandIdentityContext>
        </CatalogToastContext>
      </CatalogDetailDialogContext>
    </CommandDialogContext>
  );
}

function CatalogCommandDialog(props: {
  readonly input: CommandDialogInput;
  readonly dialog: Cell<CommandDialogInput | null>;
}): JSX.Element {
  const input = () => props.input;
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const session = useContext(PublicSessionResourceContext);
  const identity = useContext(PublicCommandIdentityContext);
  const authenticated = () => {
    if (session.state() !== "ready") {
      return false;
    }
    const snapshot = session();
    if (snapshot === undefined || !snapshot.ok || snapshot.value === null) {
      return false;
    }
    return (
      snapshot.value.authenticated &&
      canonicalUuid(snapshot.value.user_id) === canonicalUuid(identity.userId)
    );
  };
  const toasts = useContext(CatalogToastContext);
  // 代理成功投影与命令状态都由稳定宿主持有；表单替换不能取消任务或丢失结果。
  const proxySuccess = createCell<ProxyCommandAuthorization | null>(null);
  // 身份宿主销毁会连带丢弃结果未知的冻结命令；这是与 Rust 实现一致的行为，
  // 如需保留恢复记录，必须两套实现同时调整。
  const bidCommand = useConfirmCommand<PendingBidCommand, CommandError>();
  const proxyCreateCommand = useConfirmCommand<PendingProxyCommand, CommandError>();
  const proxyLowerCommand = useConfirmCommand<PendingProxyCommand, CommandError>();
  const busy = () =>
    bidCommand.submitting() || proxyCreateCommand.submitting() || proxyLowerCommand.submitting();
  // 二次确认是确认视图的唯一状态来源；不复制到本地信号，避免命令被替换后
  // 仍显示旧确认行。结果未知时同样隐藏切换条，恢复入口不能被换成另一类命令。
  const confirming = () =>
    bidCommand.confirmation() !== null ||
    proxyCreateCommand.confirmation() !== null ||
    proxyLowerCommand.confirmation() !== null;
  const recovering = () =>
    bidCommand.uncertain() || proxyCreateCommand.uncertain() || proxyLowerCommand.uncertain();
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
  // 普通出价被接受后直接收起弹窗，结果改由页面右下角的提示条回显。此时命令状态
  // 已由 ConfirmCommand 清空（成功即终态），关闭不会丢弃冻结的 command_id 或未知结果；
  // 出价事实来自服务端响应，提示条只回显它，目录行随后由权威投影刷新。
  const bidSucceeded = (success: BidSuccess) => {
    const price = formatCreditAmount(success.currentPrice);
    const message = success.leading ? t("bid-success-leading", { price }) : t("bid-success");
    const detail = success.leading ? null : t("bid-current-price", { price });
    toasts.show(input().lotCode, message, detail);
    props.dialog.set(null);
  };
  // 切换 tab 走同一弹窗快照：actions.rs 的代理资源与刷新订阅据此激活，不另建状态。
  const switchTab = (action: CatalogAction) => {
    const next = props.dialog.peek();
    if (next === null || next.action === action) {
      return;
    }
    props.dialog.set({ ...next, action });
  };
  const title = () =>
    confirming()
      ? t("bid-confirm-title")
      : input().action === "bid"
        ? t("bid-dialog-title", { code: input().lotCode })
        : t("proxy-dialog-title", { code: input().lotCode });
  const canBid = () => input().canBid && authenticated();
  const proxyInput = (): ProxyFormInput => ({
    lotId: input().lotId,
    canBid: canBid(),
    currentPrice: input().currentPrice,
    nextBid: input().nextBid,
    lotStatus: input().lotStatus,
    allowSlow: input().allowSlow,
    defaultMode: input().proxyDefaultMode,
    leading: input().leading,
    blockedReason: input().blockedReason,
    proxy: input().proxy,
    refreshGeneration,
    savedAuthorization: proxySuccess,
    createCommand: proxyCreateCommand,
    lowerCommand: proxyLowerCommand,
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
              <Show when={!recovering()}>
                {/* 直接出价/代理出价共用同一弹窗；切换只替换表单内容，不新建命令状态。 */}
                {/* biome-ignore lint/a11y/useSemanticElements: 源码使用 div role=group 保持 DOM 结构。 */}
                <div class="public-bid-dialog__tabs" role="group" aria-label={t("bid-type-label")}>
                  <button
                    class={
                      input().action === "bid"
                        ? "public-bid-dialog__tab public-bid-dialog__tab--active"
                        : "public-bid-dialog__tab"
                    }
                    type="button"
                    aria-pressed={input().action === "bid" ? "true" : "false"}
                    disabled={busy()}
                    onClick={() => switchTab("bid")}
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
        <Switch fallback={null}>
          <Match when={input().action === "bid"}>
            <Show
              when={bidCommand.uncertain()}
              fallback={
                <BidForm
                  input={{
                    lotId: input().lotId,
                    nextBid: input().nextBid,
                    lotStatus: input().lotStatus,
                    canBid: canBid(),
                    blocked: input().ordinaryBlocked,
                    blockedReason: input().blockedReason,
                    command: bidCommand,
                    onSuccess: bidSucceeded,
                    onClose: close,
                  }}
                />
              }
            >
              <BidRetry
                lotId={input().lotId}
                lotCode={input().lotCode}
                command={bidCommand}
                onSuccess={bidSucceeded}
                onClose={close}
              />
            </Show>
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
