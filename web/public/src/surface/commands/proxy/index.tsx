// Public 代理命令组件的稳定出口。
// 入口只负责根据服务端投影选择创建、降低上限或成功反馈视图；各表单自己的状态和
// 确认逻辑放在各表单，已确认的负载交给外壳。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Ok, Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, Match, Show, Switch } from "solid-js";

import type { OwnedProxy } from "../../../api/account";
import type { PendingProxyCommand, ProxyConfirmCommand } from "../pending";
import { ProxyAuthorizationForm } from "./authorization";
import { ProxyCreateForm } from "./create";

/** 公开 lot 的代理授权表单参数；账户状态由服务端投影，不在浏览器端复制代理状态机。 */
export interface ProxyFormInput {
  readonly lotId: string;
  readonly authenticated: boolean;
  readonly canBid: boolean;
  readonly canSubmit: boolean;
  readonly currentPrice: number;
  readonly nextBid: number;
  readonly lotStatus: string;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式，决定新建表单的初始选项；未知时新建表单失败关闭。 */
  readonly defaultMode: string | null;
  readonly leading: boolean;
  readonly blockedReason: string;
  readonly proxy: Result<OwnedProxy | null, string> | null;
  readonly refreshGeneration: Cell<number>;
  /** 由弹窗持有的创建和降低上限的确认草稿。 */
  readonly createCommand: ProxyConfirmCommand;
  readonly lowerCommand: ProxyConfirmCommand;
  readonly onSubmit: (pending: PendingProxyCommand) => void;
  readonly onClose: () => void;
}

/** 公开 lot 的代理授权入口；输入以普通值传入，响应式读取留在组件内部。 */
export function ProxyForm(props: { readonly input: ProxyFormInput }): JSX.Element {
  const input = () => props.input;
  const proxy = () => input().proxy;
  const proxyRead = useProxyDraftRead(proxy);
  const proxyError = () => proxyRead().error;
  const confirmed = () => proxyRead().confirmed;
  const activeProxy = () => activeOwnedProxy(confirmed());
  // 草稿沿用上次授权快照。新的写入仍需当前授权读取和会话许可，失败时不能使用旧上限。
  const canSubmit = () => input().canBid && input().canSubmit && proxyRead().canSubmit;
  const initialMinimum = () => {
    const previous = confirmed()?.value;
    return previous == null
      ? input().nextBid
      : Math.max(input().nextBid, saturatingAddOne(previous.maximum_amount));
  };
  const retryProxy = () => {
    const generation = input().refreshGeneration;
    generation.set(nextRefreshGeneration(generation()));
  };

  return (
    <Switch
      fallback={
        <>
          <Show when={!input().canBid}>
            <p role="status">{t("bid-unavailable")}</p>
          </Show>
          <Show when={input().canBid && proxy() === null}>
            <p role="status">{t("proxy-loading")}</p>
          </Show>
          <Show when={proxyError() !== null}>
            <AsyncState
              kind="error"
              title={t("catalog-proxy")}
              detail={t("proxy-load-error")}
              retryLabel={t("data-retry")}
              onRetry={retryProxy}
            />
          </Show>
          <Switch fallback={null}>
            <Match when={activeProxy()}>
              {(proxy) => (
                <ProxyAuthorizationForm
                  lotId={input().lotId}
                  currentPrice={input().currentPrice}
                  nextBid={input().nextBid}
                  lotStatus={input().lotStatus}
                  proxy={proxy()}
                  canSubmit={canSubmit()}
                  command={input().lowerCommand}
                  onSubmit={input().onSubmit}
                  onClose={input().onClose}
                />
              )}
            </Match>
            <Match when={confirmed() !== null}>
              <ProxyCreateForm
                lotId={input().lotId}
                initialMinimum={initialMinimum()}
                lotStatus={input().lotStatus}
                allowSlow={input().allowSlow}
                defaultMode={input().defaultMode}
                canSubmit={canSubmit()}
                blocked={input().leading}
                blockedReason={input().blockedReason}
                command={input().createCommand}
                onSubmit={input().onSubmit}
                onClose={input().onClose}
              />
            </Match>
          </Switch>
        </>
      }
    >
      <Match when={!input().authenticated || (!input().canBid && confirmed() === null)}>
        <p role="status">{t("bid-unavailable")}</p>
      </Match>
    </Switch>
  );
}

/** 弹窗按已确认身份挂载，沿用这一份表单投影，直到新的权威授权结果替换它。 */
export function useProxyDraftRead(snapshot: () => Result<OwnedProxy | null, string> | null) {
  let confirmed: Ok<OwnedProxy | null> | null = null;
  return createValueMemo(() => {
    const current = snapshot();
    if (current?.ok === true) {
      confirmed = current;
    }
    return {
      confirmed,
      error: current?.ok === false ? current.error : null,
      canSubmit: current?.ok === true,
    };
  });
}

/** 只使用已读取的本人活动授权；是否生效由服务端投影决定。 */
export function activeOwnedProxy(
  snapshot: Result<OwnedProxy | null, string> | null,
): OwnedProxy | null {
  return snapshot?.ok === true && snapshot.value?.status === "active" ? snapshot.value : null;
}

/** 辅助函数适配访问器调用点；组件本身只接收普通输入值。 */
export function proxyForm(input: Accessor<ProxyFormInput>): JSX.Element {
  return <ProxyForm input={input()} />;
}

function saturatingAddOne(value: number): number {
  return value >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : value + 1;
}

function nextRefreshGeneration(value: number): number {
  return value >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : value + 1;
}
