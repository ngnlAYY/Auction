// Admin 邮件页面加载、筛选和整体布局。

import type { QueryState } from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import type { AdminMailIntakeDetail, AdminMailIntakePage, AdminMailPolling } from "../../api/mail";
import { fetchMailIntakeDetail, fetchMailIntakes, fetchMailState } from "../../api/mail";
import { useAdminSessionHost } from "../auth/shell";
import { actionFeedbackMessage } from "../deliveries/labels";
import { useAdminPage, useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import {
  type AdminActionFeedback,
  adminActionFeedbackIsSuccess,
  adminApiErrorMessage,
  adminWorldLabel,
} from "../presentation";
import { type MailQuery, mailQueryToIntakeQuery, useMailQueryState } from "../query/mail";
import { MailPollingCard, MailPollingCommands, MailTriggerButton } from "./actions";
import { MailIntakeDetailPanel } from "./detail";
import { MailIntakeTable } from "./table";

/** 一次邮箱轮询读取；邮箱与世代随结果一起保留，供页面丢弃迟到快照。 */
interface PollingRead {
  readonly mailbox: string;
  readonly generation: bigint;
  readonly result: Result<AdminMailPolling | null, string>;
}

/** 一次入站详情读取；收件身份随结果一起保留，资源重跑期间旧值不能装配面板。 */
interface DetailRead {
  readonly intakeId: string | null;
  readonly result: Result<AdminMailIntakeDetail | null, string>;
}

type IntakeView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminMailIntakePage };

interface DetailView {
  readonly detail: AdminMailIntakeDetail | null;
  readonly loading: boolean;
  readonly error: string | null;
}

type PollingView =
  | { readonly kind: "idle" }
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "card"; readonly polling: AdminMailPolling; readonly generation: bigint }
  | { readonly kind: "not-loaded" };

/** 加载并展示指定邮箱的轮询状态和入站邮件摘要。 */
export function page(): JSX.Element {
  // 入站列表的世界/类型/状态筛选与分页跟地址栏双向同步，刷新/分享/前进后退可恢复；
  // 邮箱轮询输入、详情面板开合和操作反馈是瞬态，仍由 signal 持有、不入 URL。
  const host = useAdminSessionHost();
  const query = useMailQueryState();
  const queryState = query.state;
  const requestedQuery = createValueMemo(() => {
    const current = queryState();
    return { query: mailQueryToIntakeQuery(current), page: current.page } as const;
  });
  const mailboxKey = createCell("");
  const activeMailbox = createCell("");
  const refresh = createCommandReload(0n);
  const pollingRefresh = createCommandReload(0n);
  // 重排反馈由页面持有，详情刷新或切换期间不会把成功排队误报为已送达。
  const deliveryNotice = createCell<readonly [string, AdminActionFeedback] | null>(null);
  const selectedIntake = createCell<string | null>(null);

  const polling = createTaskResource(
    () => [pollingRefresh(), activeMailbox()] as const,
    async ([generation, mailbox], context): Promise<PollingRead> => {
      if (mailbox === "") {
        return { mailbox, generation, result: ok(null) };
      }
      const fetched = await fetchMailState(host, mailbox, context.signal);
      return {
        mailbox,
        generation,
        result: fetched.ok ? ok(fetched.value) : err(fetched.error),
      };
    },
  );
  const pagination = useAdminPagination("mail", ["page_size"], 100, 2_147_483_647);
  const intakes = useAdminPage(
    pagination,
    () => {
      refresh();
      return requestedQuery();
    },
    (request, size, signal) => fetchMailIntakes(host, request.query, request.page, size, signal),
  );
  const detail = createTaskResource(
    () => [refresh(), selectedIntake()] as const,
    async ([, intakeId], context): Promise<DetailRead> => {
      if (intakeId === null) {
        return { intakeId, result: ok(null) };
      }
      const fetched = await fetchMailIntakeDetail(host, intakeId, context.signal);
      if (!fetched.ok) {
        return { intakeId, result: err(fetched.error) };
      }
      return fetched.value.id === intakeId
        ? { intakeId, result: ok(fetched.value) }
        : { intakeId, result: err("invalid_response") };
    },
  );
  const deliveryCommands = usePendingCommands(refresh, (entity) => {
    const prefix = "delivery:";
    if (!entity.startsWith(prefix)) {
      return false;
    }
    const deliveryId = entity.slice(prefix.length);
    const selected = selectedIntake.peek();
    if (selected === null) {
      return false;
    }
    const snapshot = detail.peek();
    if (snapshot === undefined || snapshot.intakeId !== selected) {
      return true;
    }
    if (!snapshot.result.ok) {
      return true;
    }
    const value = snapshot.result.value;
    return value === null || value.deliveries.some((delivery) => delivery.id === deliveryId);
  });

  const loadPolling = (event: SubmitEvent): void => {
    event.preventDefault();
    const nextMailbox = rustTrim(mailboxKey());
    if (nextMailbox === "") {
      return;
    }
    activeMailbox.set(nextMailbox);
    advanceCommandReload(pollingRefresh);
  };

  // 三个闭集筛选各持一份 hook 克隆；克隆共享同一状态信号。
  const worldQuery = query;
  const statusQuery = query;
  const kindQuery = query;

  const intakeView = createValueMemo((): IntakeView => {
    const current = intakes();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", code: current.error };
    }
    return { kind: "ready", page: current.value };
  });
  const intakeError = createValueMemo((): { readonly code: string } | null => {
    const current = intakeView();
    return current.kind === "error" ? current : null;
  });
  const intakePage = createValueMemo(() => {
    const current = intakeView();
    return current.kind === "ready" ? current.page : null;
  });
  // Resource 重启会保留旧值；只有同一收件的快照可继续展示和操作。
  const detailView = createValueMemo((): DetailView => {
    const snapshot = detail();
    if (snapshot === undefined || snapshot.intakeId !== selectedIntake()) {
      return { detail: null, loading: true, error: null };
    }
    if (!snapshot.result.ok) {
      return { detail: null, loading: false, error: snapshot.result.error };
    }
    return { detail: snapshot.result.value, loading: false, error: null };
  });
  // 只保留当前邮箱的完成快照；切换邮箱不能继续显示或操作上一邮箱的轮询状态。
  const pollingView = createValueMemo((): PollingView => {
    const mailbox = activeMailbox();
    if (mailbox === "") {
      return { kind: "idle" };
    }
    const snapshot = polling();
    if (snapshot === undefined || snapshot.mailbox !== mailbox) {
      return { kind: "loading" };
    }
    if (!snapshot.result.ok) {
      return { kind: "error", code: snapshot.result.error };
    }
    const value = snapshot.result.value;
    return value === null
      ? { kind: "not-loaded" }
      : { kind: "card", polling: value, generation: snapshot.generation };
  });
  const pollingCard = createValueMemo(() => {
    const current = pollingView();
    return current.kind === "card" ? current : null;
  });
  const pollingError = createValueMemo((): { readonly code: string } | null => {
    const current = pollingView();
    return current.kind === "error" ? current : null;
  });

  return (
    <PendingCommandsContext value={deliveryCommands}>
      <section class="admin-mail-page" aria-labelledby="mail-title">
        <h1 id="mail-title">{t("page-mails-title")}</h1>
        <PendingCommandPanel owner={deliveryCommands} />
        <div class="admin-mail-filters">
          <label for="mail-world">
            {t("admin-mail-intake-world")}
            <select
              id="mail-world"
              name="world"
              autocomplete="off"
              class="admin-mail-world"
              value={queryState().world}
              onChange={(event) => {
                const value = event.currentTarget.value;
                applyMailFilter(worldQuery, selectedIntake, (next) => {
                  next.world = value;
                });
              }}
            >
              <option value="">{t("admin-mail-all")}</option>
              <option value="main">{adminWorldLabel("main")}</option>
              <option value="isekai">{adminWorldLabel("isekai")}</option>
            </select>
          </label>
          <label for="mail-status">
            {t("admin-mail-intake-status")}
            <select
              id="mail-status"
              name="status"
              autocomplete="off"
              class="admin-mail-state-filter"
              value={queryState().status}
              onChange={(event) => {
                const value = event.currentTarget.value;
                applyMailFilter(statusQuery, selectedIntake, (next) => {
                  next.status = value;
                });
              }}
            >
              <option value="">{t("admin-mail-all")}</option>
              <option value="accepted">{t("admin-mail-filter-status-accepted")}</option>
              <option value="rejected">{t("admin-mail-filter-status-rejected")}</option>
              <option value="failed">{t("admin-mail-filter-status-failed")}</option>
            </select>
          </label>
          <label for="mail-kind">
            {t("admin-mail-intake-kind")}
            <select
              id="mail-kind"
              name="kind"
              autocomplete="off"
              class="admin-mail-kind"
              value={queryState().kind}
              onChange={(event) => {
                const value = event.currentTarget.value;
                applyMailFilter(kindQuery, selectedIntake, (next) => {
                  next.kind = value;
                });
              }}
            >
              <option value="">{t("admin-mail-all")}</option>
              <option value="equipment">{t("admin-mail-filter-kind-equipment")}</option>
              <option value="bid">{t("admin-mail-filter-kind-bid")}</option>
              <option value="delivery">{t("admin-mail-filter-kind-delivery")}</option>
              <option value="registration">{t("admin-mail-filter-kind-registration")}</option>
              <option value="rebind">{t("admin-mail-filter-kind-rebind")}</option>
              <option value="rekey">{t("admin-mail-kind-rekey")}</option>
              <option value="unknown">{t("admin-mail-filter-kind-unknown")}</option>
            </select>
          </label>
        </div>
        <Switch fallback={<AsyncState kind="loading" title={t("admin-mail-loading")} detail="" />}>
          <Match when={intakeError()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(refresh)}
              />
            )}
          </Match>
          <Match when={intakePage()}>
            {(page) => (
              <MailIntakeTable
                page={page()}
                pagination={pagination}
                sort={queryState().sort}
                direction={queryState().direction}
                onPageChange={(nextPage) => {
                  const next: MailQuery = { ...queryState(), page: BigInt(nextPage) };
                  query.push(next);
                }}
                onSortChange={(value) => {
                  const next: MailQuery = {
                    ...queryState(),
                    sort: value[0],
                    direction: value[1],
                  };
                  query.push(next);
                }}
                onSelect={(intakeId) => {
                  deliveryNotice.set(null);
                  selectedIntake.set(intakeId);
                }}
              />
            )}
          </Match>
        </Switch>
        <MailIntakeDetailPanel
          visible={selectedIntake() !== null}
          detail={detailView().detail}
          loading={detailView().loading}
          error={detailView().error}
          refresh={refresh}
          deliveryNotice={deliveryNotice}
          onClose={() => {
            deliveryNotice.set(null);
            selectedIntake.set(null);
          }}
        />
        <Show when={deliveryNotice()}>
          {(notice) => (
            <Show when={selectedIntake() === notice()[0]}>
              <p
                class={[
                  "admin-panel__note",
                  "action-feedback",
                  adminActionFeedbackIsSuccess(notice()[1])
                    ? "action-feedback--success"
                    : "action-feedback--error",
                ]}
                role={adminActionFeedbackIsSuccess(notice()[1]) ? "status" : "alert"}
              >
                {actionFeedbackMessage(notice()[1], t("admin-mail-delivery-requeued"))}
              </p>
            </Show>
          )}
        </Show>
        <details class="admin-mail-polling">
          <summary id="mail-polling-title">{t("admin-mail-polling-title")}</summary>
          <form class="admin-mail-search" onSubmit={loadPolling}>
            <label for="mailbox-key">{t("admin-mail-mailbox")}</label>
            <input
              id="mailbox-key"
              name="mailbox_key"
              autocomplete="off"
              autocapitalize="none"
              spellcheck="false"
              enterkeyhint="search"
              maxlength="256"
              required={true}
              value={mailboxKey()}
              onInput={(event) => mailboxKey.set(event.currentTarget.value)}
            />
            <button type="submit">{t("admin-mail-load")}</button>
          </form>
          <MailPollingCommands refresh={pollingRefresh} activeMailbox={activeMailbox}>
            <MailTriggerButton />
            <Switch
              fallback={<AsyncState kind="loading" title={t("admin-mail-loading")} detail="" />}
            >
              <Match when={pollingView().kind === "idle" || pollingView().kind === "not-loaded"}>
                <p>{t("admin-mail-polling-not-loaded")}</p>
              </Match>
              <Match when={pollingError()}>
                {(error) => (
                  <AsyncState
                    kind="error"
                    title={adminApiErrorMessage(error().code)}
                    detail=""
                    retryLabel={t("admin-retry")}
                    onRetry={() => advanceCommandReload(pollingRefresh)}
                  />
                )}
              </Match>
              <Match when={pollingCard()}>
                {(card) => (
                  <For each={[card()]} keyed={(value) => value.polling.mailbox_key}>
                    {(value) => (
                      <MailPollingCard
                        polling={value().polling}
                        refresh={pollingRefresh}
                        loadedGeneration={value().generation}
                      />
                    )}
                  </For>
                )}
              </Match>
            </Switch>
          </MailPollingCommands>
        </details>
      </section>
    </PendingCommandsContext>
  );
}

/** 闭集筛选选择变化后立即生效：push 到地址栏并回到第一页，同时清理详情——
 * 旧选择不再属于新结果集。查询资源响应 `query.state` 变化自动重新加载，无需手动刷新。 */
function applyMailFilter(
  query: QueryState<MailQuery>,
  selectedIntake: Cell<string | null>,
  update: (next: MailQuery) => void,
): void {
  const next: MailQuery = { ...query.state() };
  update(next);
  next.page = 1n;
  query.push(next);
  selectedIntake.set(null);
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 会漏掉 U+0085 等字符，不能替换。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
