// Realtime failure 表格与分页组件。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { utf8ByteLength } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { RealtimeFailure, RealtimeFailurePage } from "../../api/realtime";
import { requirePendingCommands } from "../pending";

interface FailureRow {
  readonly failure: RealtimeFailure;
  readonly errorDetail: string;
}

/** 展示可供操作员选择 replay 的 failure 安全投影。 */
export function RealtimeFailureTable(props: {
  readonly page: RealtimeFailurePage;
  readonly onSelect: (failure: RealtimeFailure) => void;
}): JSX.Element {
  const pending = requirePendingCommands();
  // 操作负载与错误详情在渲染循环外配好，循环体根节点才能直接是带 key 的行。
  const rows = createValueMemo((): readonly FailureRow[] =>
    props.page.failures.map((failure) => ({
      failure,
      errorDetail: `${failure.code}: ${failure.safe_detail}`,
    })),
  );

  return (
    <>
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-realtime-table"
          aria-label={t("realtime-failure-title")}
        >
          <TableColumns layout="admin-realtime-failures" widths={[17, 20, 17, 9, 12, 12, 7, 6]} />
          <thead>
            <tr>
              <th scope="col">{t("realtime-failure-time")}</th>
              <th scope="col">{t("realtime-failure-error")}</th>
              <th scope="col">{t("realtime-stream")}</th>
              <th scope="col">{t("realtime-consumer")}</th>
              <th scope="col">{t("realtime-failure-event-id")}</th>
              <th scope="col">{t("realtime-failure-id")}</th>
              <th scope="col">{t("realtime-failure-state")}</th>
              <th scope="col">{t("catalog-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={rows()} keyed={(row) => row.failure.id}>
              {(row) => (
                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                <tr class="admin-realtime__row">
                  <td>
                    <Time value={row().failure.failed_at} />
                  </td>
                  <td class="admin-realtime__error">
                    <TruncatedText
                      display={row().failure.code}
                      size="wide"
                      value={row().errorDetail}
                    />
                  </td>
                  <td>
                    <TruncatedText value={row().failure.stream_key} />
                  </td>
                  <td>
                    <TruncatedText value={row().failure.consumer_key} />
                  </td>
                  <td>
                    <TruncatedText
                      display={compactRealtimeId(row().failure.event_id)}
                      size="compact"
                      value={row().failure.event_id}
                    />
                  </td>
                  <td>
                    <TruncatedText
                      display={compactRealtimeId(row().failure.id)}
                      size="compact"
                      value={row().failure.id}
                    />
                  </td>
                  <td>
                    <span
                      class={
                        row().failure.resolved
                          ? "admin-realtime__status admin-realtime__status--resolved"
                          : "admin-realtime__status admin-realtime__status--unresolved"
                      }
                    >
                      {row().failure.resolved
                        ? t("realtime-state-completed")
                        : t("realtime-state-dead-letter")}
                    </span>
                  </td>
                  <td>
                    <div class="admin-realtime__actions">
                      <Show when={!row().failure.resolved}>
                        <button
                          type="button"
                          disabled={
                            pending.anyBusy() ||
                            pending.contains(`realtime-failure:${row().failure.id}`)
                          }
                          // 与 Rust 每次点击交出独立副本一致，回调修改不会回写行内 DTO。
                          onClick={() => props.onSelect({ ...row().failure })}
                        >
                          {t("realtime-replay")}
                        </button>
                      </Show>
                    </div>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={props.page.failures.length === 0}>
        <AsyncState kind="empty" title={t("realtime-failure-empty")} detail="" />
      </Show>
    </>
  );
}

function compactRealtimeId(value: string): string {
  if (utf8ByteLength(value) <= 12) {
    return value;
  }
  return Array.from(value).slice(-8).join("");
}
