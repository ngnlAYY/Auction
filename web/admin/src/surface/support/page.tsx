// 玩家支持视图：按用户名/用户 ID 或参考编号只读汇总。
//
// 页面只发一次有界只读查询，不提供任何写动作；输入与结果都不缓存到浏览器本地。
// 检索入口互斥，提交前由 URL 状态与页面共同校验，服务端仍会再次失败关闭。

import type { AdminPlayerSupportBundleResponse } from "@auction/contracts/dto/admin/delivery";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, Show, Switch } from "solid-js";

import {
  fetchPlayerSupport,
  MAX_PLAYER_SUPPORT_USER_BYTES,
  playerSupportUserTooLong,
} from "../../api/delivery";
import { trimRustWhitespace } from "../../api/session-time";
import { useAdminSessionHost } from "../auth/shell";
import { adminApiErrorMessage } from "../presentation";
import { type SupportQuery, supportQueryValid, useSupportQueryState } from "../query/support";
import { FieldError, useFocusField } from "../validation";
import { bids, deals, deliveries, notifications, obligations, summary } from "./sections";

interface SupportLoad {
  readonly query: SupportQuery;
  readonly result: Result<AdminPlayerSupportBundleResponse, string>;
}

/** 路由入口。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  // 已应用的检索入口只由地址栏持有：提交 push、前进/后退由 hook 恢复，资源直接订阅它。
  const query = useSupportQueryState();
  const queryState = query.state;
  const userInput = createCell(queryState.peek().user);
  const referenceInput = createCell(queryState.peek().reference);
  const invalid = createCell(false);
  const invalidUser = createCell(playerSupportUserTooLong(queryState.peek().user));
  const focusField = useFocusField();
  // 输入框是提交前草稿；已应用入口变化（提交或前进/后退）时重置为当前值，与地址栏一致。
  createEffect(
    () => queryState(),
    (applied) => {
      if (userInput.peek() !== applied.user) {
        userInput.set(applied.user);
      }
      if (referenceInput.peek() !== applied.reference) {
        referenceInput.set(applied.reference);
      }
      if (invalid.peek()) {
        invalid.set(false);
      }
      const userTooLong = playerSupportUserTooLong(applied.user);
      if (invalidUser.peek() !== userTooLong) {
        invalidUser.set(userTooLong);
      }
    },
  );

  // 只接受恰好一个入口；参考编号必须是规范 UUID，用户 ID 或用户名都交给服务端解析。
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    const next: SupportQuery = {
      user: trimRustWhitespace(userInput.peek()),
      reference: trimRustWhitespace(referenceInput.peek()),
    };
    const userTooLong = playerSupportUserTooLong(next.user);
    invalidUser.set(userTooLong);
    if (userTooLong) {
      invalid.set(false);
      focusField("support-user");
      return;
    }
    if (!supportQueryValid(next)) {
      invalid.set(true);
      return;
    }
    invalid.set(false);
    query.push(next);
  };

  const resource = createTaskResource(
    () => queryState(),
    async (current, context): Promise<SupportLoad | null> => {
      if (!supportQueryValid(current)) {
        return null;
      }
      const result = await fetchPlayerSupport(
        host,
        current.user,
        current.reference,
        context.signal,
      );
      if (!context.alive()) return null;
      return { query: current, result };
    },
  );
  // 结果必须与当前入口一致；地址栏变化后不展示上一轮汇总。
  const current = createValueMemo(() => queryState());
  const addressUserTooLong = createValueMemo(() => playerSupportUserTooLong(current().user));
  const loading = createValueMemo(() => !addressUserTooLong() && resource.state() !== "ready");
  const result = createValueMemo((): Result<AdminPlayerSupportBundleResponse, string> | null => {
    if (addressUserTooLong() || loading()) {
      return null;
    }
    const entry = resource();
    if (entry === undefined || entry === null) {
      return null;
    }
    if (!supportQueryEquals(entry.query, current())) {
      return null;
    }
    return entry.result;
  });
  const failed = createValueMemo((): { readonly code: string } | null => {
    const value = result();
    return value !== null && !value.ok ? { code: value.error } : null;
  });
  const bundle = createValueMemo((): AdminPlayerSupportBundleResponse | null => {
    const value = result();
    return value?.ok ? value.value : null;
  });

  return (
    <section class="admin-support-page" aria-labelledby="support-title">
      <h1 id="support-title">{t("page-support-title")}</h1>
      <form class="admin-delivery-lookup" onSubmit={submit}>
        <label for="support-user">{t("support-user")}</label>
        <input
          id="support-user"
          type="text"
          aria-invalid={invalidUser() ? "true" : "false"}
          aria-describedby={invalidUser() ? "support-user-error" : undefined}
          value={userInput()}
          onInput={(event) => {
            const value = event.currentTarget.value;
            if (invalidUser.peek() && !playerSupportUserTooLong(trimRustWhitespace(value))) {
              invalidUser.set(false);
            }
            userInput.set(value);
          }}
        />
        <FieldError
          id="support-user-error"
          message={
            invalidUser()
              ? t("admin-field-max-utf8-bytes", { max: MAX_PLAYER_SUPPORT_USER_BYTES })
              : null
          }
        />
        <label for="support-reference">{t("support-search-reference")}</label>
        <input
          id="support-reference"
          type="text"
          maxlength="36"
          value={referenceInput()}
          onInput={(event) => referenceInput.set(event.currentTarget.value)}
        />
        <button type="submit">{t("support-search")}</button>
        <Show when={invalid()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("support-invalid-query")}
          </p>
        </Show>
      </form>
      <Switch>
        <Match when={result() === null && loading()}>
          <AsyncState kind="loading" title={t("data-loading")} detail="" />
        </Match>
        <Match when={result() === null && !invalidUser() && !addressUserTooLong()}>
          <p class="admin-panel__note">{t("support-idle")}</p>
        </Match>
        <Match when={failed()}>
          {(failure) => (
            <AsyncState kind="error" title={adminApiErrorMessage(failure().code)} detail="" />
          )}
        </Match>
        <Match when={bundle()}>
          {(value) => (
            <>
              {summary(value())}
              {bids(value().bids)}
              {deals(value().deals)}
              {obligations(value().obligations)}
              {deliveries(value().deliveries)}
              {notifications(value().notifications)}
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}

/** 与 Rust `PartialEq` 对应：只有用户与参考编号都相同才算同一个已应用入口。 */
function supportQueryEquals(left: SupportQuery, right: SupportQuery): boolean {
  return left.user === right.user && left.reference === right.reference;
}
