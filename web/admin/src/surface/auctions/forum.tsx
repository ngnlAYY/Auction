// 拍卖管理中的论坛状态与正文诊断；只读 GET，不代替人工写入恢复授权。

import type { PublicForumResponse } from "@auction/contracts/dto/public/catalog/forum";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminAuction } from "../../api/auctions";
import { type AdminForumStatus, fetchAdminForumStatus } from "../../api/forum";
import { useAdminSessionHost } from "../auth/shell";
import { adminApiErrorMessage, adminWorldLabel } from "../presentation";

/** 从当前列表页选择拍卖，保留列表筛选和表格列宽。 */
export function AdminAuctionForumStatus(props: {
  readonly auctions: readonly AdminAuction[];
}): JSX.Element {
  const selected = createCell("");
  const selectedAuction = createValueMemo(
    () => props.auctions.find((auction) => auction.id === selected()) ?? props.auctions[0] ?? null,
  );
  return (
    <Show when={selectedAuction()}>
      {(auction) => (
        <section class="admin-forum-status" aria-labelledby="admin-forum-title">
          <h2 id="admin-forum-title">{t("admin-forum-title")}</h2>
          <p>{t("admin-forum-readonly-note")}</p>
          <label for="admin-forum-auction">{t("admin-forum-auction")}</label>
          <select
            id="admin-forum-auction"
            value={auction().id}
            onChange={(event) => selected.set(event.currentTarget.value)}
          >
            <For each={props.auctions} keyed={false}>
              {(item) => (
                <option value={item().id}>
                  {t("admin-auctions-row-heading", {
                    number: item().sequence,
                    world: adminWorldLabel(item().world_id),
                  })}
                </option>
              )}
            </For>
          </select>
          <ForumStatusReadout auctionId={auction().id} />
        </section>
      )}
    </Show>
  );
}

interface ForumReadout {
  readonly auctionId: string;
  readonly value: Result<AdminForumStatus, string>;
}

function ForumStatusReadout(props: { readonly auctionId: string }): JSX.Element {
  const host = useAdminSessionHost();
  const generation = createCell(0);
  // 普通 prop 不会自动重启资源；显式订阅拍卖身份，不依赖静态组件的 key。
  // 列表刷新会带来新的场次对象，即使 ID 与刷新世代都没变；先按值去重，
  // 同一场次的 `state_version` 等字段更新不能让论坛读取重新进入加载态。
  const dependencies = createValueMemo(() => [props.auctionId, generation()] as const);
  const status = createTaskResource(
    dependencies,
    async ([auctionId], context): Promise<ForumReadout | null> => {
      const value = await fetchAdminForumStatus(host, auctionId, context.signal);
      if (!context.alive()) {
        return null;
      }
      return { auctionId, value };
    },
  );
  const busy = () => status.state() !== "ready";
  // 正在刷新或切换拍卖时不显示上一轮结果，避免把旧观测展示在新拍卖下。
  const result = createValueMemo(
    (): { readonly value: Result<AdminForumStatus, string> } | null => {
      if (busy()) {
        return null;
      }
      const loaded = status();
      if (loaded === undefined || loaded === null || loaded.auctionId !== props.auctionId) {
        return null;
      }
      return { value: loaded.value };
    },
  );
  return (
    <>
      <button
        type="button"
        disabled={busy()}
        onClick={() => {
          generation.update((value) => (value < Number.MAX_SAFE_INTEGER ? value + 1 : value));
        }}
      >
        {t("admin-forum-refresh")}
      </button>
      <Show when={result()} fallback={<p>{t("admin-forum-loading")}</p>}>
        {(loaded) => <ForumReadoutResult value={loaded().value} />}
      </Show>
    </>
  );
}

function ForumReadoutResult(props: {
  readonly value: Result<AdminForumStatus, string>;
}): JSX.Element {
  const success = createValueMemo(() => (props.value.ok ? props.value.value : null));
  const failure = createValueMemo(() => (props.value.ok ? null : { code: props.value.error }));
  return (
    <Show
      when={failure()}
      fallback={<Show when={success()}>{(status) => <>{forumDetails(status())}</>}</Show>}
    >
      {(error) => <p role="alert">{adminApiErrorMessage(error().code)}</p>}
    </Show>
  );
}

function forumDetails(status: AdminForumStatus): JSX.Element {
  if (!status.enabled) {
    return <p role="status">{t("admin-forum-disabled")}</p>;
  }
  const thread = status.thread;
  if (thread === null) {
    return <p role="status">{t("admin-forum-missing")}</p>;
  }
  // 译文读取必须发生在文本绑定内：在构造节点前读 `t` 会让整段正文随语言重挂载，
  // 原生 `<details>` 的展开状态也随之丢失。
  const statusLabel = (): string => {
    switch (thread.status) {
      case "pending":
        return t("admin-forum-pending");
      case "publishing":
        return t("admin-forum-publishing");
      case "published":
        return t("admin-forum-published");
      default:
        return t("admin-forum-failed");
    }
  };
  const contentDetails = forumContentDetails(thread);
  const threadUrl = thread.thread_url ?? null;
  return (
    <>
      <p role="status">{t("admin-forum-status", { status: statusLabel() })}</p>
      <dl>
        <div>
          <dt>{t("admin-forum-progress")}</dt>
          <dd>{`#${thread.processed_through_post_number}`}</dd>
        </div>
        <div>
          <dt>{t("admin-forum-updated-at")}</dt>
          <dd>
            <Time value={thread.updated_at} />
          </dd>
        </div>
      </dl>
      {threadUrl === null ? null : (
        <a href={threadUrl} target="_blank" rel="noopener noreferrer">
          {t("admin-forum-link")}
        </a>
      )}
      {contentDetails}
    </>
  );
}

/** 正文只是本地目标镜像；使用原生折叠和转义文本，不将 BBCode 当作 HTML 执行。 */
function forumContentDetails(thread: PublicForumResponse): JSX.Element {
  const contentSyncedAt = thread.content_synced_at ?? null;
  const contentLastSuccessAt = thread.content_last_success_at ?? null;
  const contentLastErrorAt = thread.content_last_error_at ?? null;
  const contentLastErrorCode = thread.content_last_error_code ?? null;
  const title = thread.title ?? null;
  const body = thread.body ?? null;
  const published =
    thread.status === "published" && title !== null && body !== null ? { title, body } : null;
  return (
    <section class="admin-forum-content" aria-labelledby="admin-forum-content-title">
      <h3 id="admin-forum-content-title">{t("admin-forum-content-title")}</h3>
      <dl>
        <div>
          <dt>{t("admin-forum-source-revision")}</dt>
          <dd>{`${thread.source_revision}`}</dd>
        </div>
        <div>
          <dt>{t("admin-forum-content-revision")}</dt>
          <dd>{`${thread.content_revision}`}</dd>
        </div>
        {contentSyncedAt === null ? null : (
          <div>
            <dt>{t("admin-forum-content-synced-at")}</dt>
            <dd>
              <Time value={contentSyncedAt} />
            </dd>
          </div>
        )}
        {contentLastSuccessAt === null ? null : (
          <div>
            <dt>{t("admin-forum-last-success-at")}</dt>
            <dd>
              <Time value={contentLastSuccessAt} />
            </dd>
          </div>
        )}
        {contentLastErrorAt === null ? null : (
          <div>
            <dt>{t("admin-forum-error-at")}</dt>
            <dd>
              <Time value={contentLastErrorAt} />
            </dd>
          </div>
        )}
      </dl>
      {contentLastErrorCode === null ? null : (
        <p class="admin-forum-content__error" role="alert">
          {t("admin-forum-content-error", { code: contentLastErrorCode })}
        </p>
      )}
      {published === null ? null : (
        <>
          <h4>{published.title}</h4>
          <p class="admin-forum-content__note">{t("admin-forum-body-note")}</p>
          <details class="admin-forum-content__body">
            <summary>{t("admin-forum-body-summary")}</summary>
            {/* biome-ignore lint/a11y/useAriaPropsSupportedByRole: 源码在可聚焦的 pre 上保留 aria-label，不能改 DOM 结构。 */}
            <pre tabindex="0" aria-label={t("admin-forum-body-summary")}>
              <code>{published.body}</code>
            </pre>
          </details>
        </>
      )}
    </section>
  );
}
