// 本人寄售页面共用的账户版式、权威身份门与安全错误文案。

import "../../../../../crates/auction-web-public/assets/consignment-account.css";

import type { ListingViewV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch, useContext } from "solid-js";

import type { ConsignmentPrivateError } from "../../api/consignment-private";
import { PublicCommandIdentityContext } from "../commands/pending";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { loginHrefForCurrentLocation } from "../session";
import { currentSessionConfirmation } from "../shell/session-authority";
import { listingWriteErrorKey } from "./labels";

/** 本人详情的站内地址，只接收由路由或契约解码确认的挂单 ID。 */
export function ownedConsignmentHref(listingId: string): string {
  return `/consignment/me/${listingId}`;
}

/** 提取安全装备投影；没有快照的状态仍可展示。 */
export function ownedEquipment(view: ListingViewV2) {
  return "equipment" in view ? view.equipment : null;
}

/** 业务码仅查显式译文，未知错误使用 Public 通用提示。 */
export function consignmentErrorMessage(error: ConsignmentPrivateError): string {
  if (error.status === 401) return t("command-session-expired");
  const key = listingWriteErrorKey(error.code);
  if (key !== null) return t(key);
  switch (error.code) {
    case "session_changed":
      return t("command-retry-session-pending");
    case "outcome_unknown":
      return t("consignment-pending");
    case "network_error":
      return t("error-network");
    case "request_timeout":
    case "timeout":
      return t("error-request-timeout");
    case "invalid_response":
      return t("error-invalid-response");
    case "command_not_sent":
      return t("command-not-sent");
    default:
      return t("data-error");
  }
}

/** 本人页读取出错，依靠重新读取恢复，且不保留旧快照。 */
export function PrivateReadError(props: {
  readonly error: ConsignmentPrivateError;
  readonly retry: () => void;
}): JSX.Element {
  return (
    <AsyncState
      kind="error"
      title={consignmentErrorMessage(props.error)}
      detail=""
      retryLabel={t("data-retry")}
      onRetry={() => props.retry()}
    />
  );
}

/** 现有账户会话未确认时不挂载表单、私有数据和命令入口。 */
export function PrivateConsignmentPage(props: {
  readonly title: string;
  readonly children: (userId: string, canUse: () => boolean) => JSX.Element;
  /** 未登录时替代登录提示的公开内容；缺省时只提示登录。 */
  readonly anonymous?: () => JSX.Element;
}): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const owner = useContext(PublicCommandOwnerContext);
  const user = () => {
    const snapshot = session();
    const value = snapshot?.ok === true ? snapshot.value : null;
    return value?.authenticated === true && owner() === value.user_id ? value.user_id : null;
  };
  const canUse = (userId: string) =>
    user() === userId && currentSessionConfirmation(session) !== null;

  return (
    <section class="public-account-bids-page public-consignment-account" aria-label={props.title}>
      <Switch>
        <Match when={session() === undefined}>
          <AsyncState kind="loading" title={t("data-loading")} detail="" />
        </Match>
        <Match when={session()?.ok === false}>
          <PrivateReadError error={{ status: 0, code: "unknown_error" }} retry={session.restart} />
        </Match>
        <Match when={session()?.ok === true && user() === null}>
          {props.anonymous?.() ?? <LoginPrompt message={t("command-login-required")} />}
        </Match>
        <Match when={user() !== null}>
          <Show when={user()} keyed>
            {(userId) => (
              <PublicCommandIdentityContext value={{ userId }}>
                {props.children(userId, () => canUse(userId))}
              </PublicCommandIdentityContext>
            )}
          </Show>
        </Match>
      </Switch>
    </section>
  );
}

/** 需要账户会话时的登录提示，登录后回到当前页面。 */
export function LoginPrompt(props: { readonly message: string }): JSX.Element {
  return (
    <div class="public-account-login-prompt" role="status">
      <p>{props.message}</p>
      <Link class="public-account-login-cta" href={loginHrefForCurrentLocation()}>
        {t("account-login-cta")}
      </Link>
    </div>
  );
}

/** 预计手续费与到手共用单位和标签；详情传入服务端的估计值。 */
export function FeeQuote(props: { readonly fee: number; readonly net: number }): JSX.Element {
  return (
    <dl class="public-consignment-account__facts">
      <div>
        <dt>{t("consignment-fee")}</dt>
        <dd>{formatCreditAmount(props.fee)}</dd>
      </div>
      <div>
        <dt>{t("consignment-net")}</dt>
        <dd>{formatCreditAmount(props.net)}</dd>
      </div>
    </dl>
  );
}
