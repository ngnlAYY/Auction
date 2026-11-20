// Realtime 事件元数据表格与分页组件。

import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { RealtimeEventPage } from "../../api/realtime";

/** 展示不含 payload 与内部来源字段的最近事件元数据。 */
export function RealtimeEventTable(props: { readonly page: RealtimeEventPage }): JSX.Element {
  return (
    <>
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-realtime-table admin-realtime-event-table"
          aria-label={t("realtime-event-title")}
        >
          <TableColumns layout="admin-realtime-events" widths={[18, 22, 26, 6, 7, 21]} />
          <thead>
            <tr>
              <th scope="col">{t("realtime-event-created-at")}</th>
              <th scope="col">{t("realtime-event-kind")}</th>
              <th scope="col">{t("realtime-stream")}</th>
              <th scope="col">{t("realtime-event-sequence")}</th>
              <th scope="col">{t("realtime-event-schema")}</th>
              <th scope="col">{t("realtime-event-id")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.page.events} keyed={(event) => event.event_id}>
              {(event) => (
                <tr class="admin-realtime__row">
                  <td>
                    <Time value={event().created_at} />
                  </td>
                  <td>
                    <TruncatedText value={event().kind} />
                  </td>
                  <td>
                    <TruncatedText value={event().stream_key} />
                  </td>
                  <td class="admin-realtime__number">{event().stream_sequence}</td>
                  <td class="admin-realtime__number">{`v${event().schema_version}`}</td>
                  <td>
                    <TruncatedText value={event().event_id} size="compact" />
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={props.page.events.length === 0}>
        <AsyncState kind="empty" title={t("realtime-event-empty")} detail="" />
      </Show>
    </>
  );
}
