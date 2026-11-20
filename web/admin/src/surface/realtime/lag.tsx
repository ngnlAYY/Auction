// Realtime lag 表格与分页组件。

import { utf8ByteLength } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { RealtimeLagPage } from "../../api/realtime";

/** 展示消费者 lag 的安全投影。 */
export function RealtimeLagTable(props: { readonly page: RealtimeLagPage }): JSX.Element {
  return (
    <Show
      when={props.page.lags.length > 0}
      fallback={<AsyncState kind="empty" title={t("realtime-lag-empty")} detail="" />}
    >
      <div class="admin-shell__table-wrap">
        <table class="admin-shell__table admin-realtime-table" aria-label={t("realtime-lag-title")}>
          <TableColumns layout="admin-realtime-lag" widths={[22, 45, 11, 12, 10]} />
          <thead>
            <tr>
              <th scope="col">{t("realtime-consumer")}</th>
              <th scope="col">{t("realtime-stream")}</th>
              <th scope="col">{t("realtime-offset")}</th>
              <th scope="col">{t("realtime-latest-sequence")}</th>
              <th scope="col">{t("realtime-lag")}</th>
            </tr>
          </thead>
          <tbody>
            <For
              each={props.page.lags}
              keyed={(lag) =>
                `${utf8ByteLength(lag.consumer_key)}:${lag.consumer_key}${lag.stream_key}`
              }
            >
              {(lag) => (
                <tr class="admin-realtime__row">
                  <td>
                    <TruncatedText value={lag().consumer_key} />
                  </td>
                  <td>
                    <TruncatedText value={lag().stream_key} />
                  </td>
                  <td>{lag().offset}</td>
                  <td>{lag().latest_sequence}</td>
                  <td>{lag().lag}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}
