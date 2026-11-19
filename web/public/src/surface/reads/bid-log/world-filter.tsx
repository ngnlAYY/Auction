// 出价记录与履约进度的世界按钮组，以及非法地址的显式恢复入口。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show, useContext } from "solid-js";

import { WorldTabs, worldLabel } from "../../board/world";
import type { PublicBoardQuery } from "../../query";
import {
  publicQueryPage,
  readPublicQueryForState,
  updatePublicQueryPairs,
} from "../../query/location";
import { WorldSwitchContext } from "../../shell/contexts";

const WORLD_ERROR_ID = "public-read-world-error";

/** 未确认的世界没有选中项；只由玩家提交可用的范围后恢复读取。 */
export function RoundWorldFilter(props: {
  readonly state: Cell<PublicBoardQuery>;
  readonly routeStillOnPage: () => boolean;
}): JSX.Element {
  const worldSwitch = useContext(WorldSwitchContext);
  const recoveryOptions = () => {
    const enabled = worldSwitch.options().filter(({ disabled }) => !disabled);
    return enabled.length === 0 ? [{ value: "all", disabled: false }] : enabled;
  };
  const recover = (value: string) => {
    if (
      !props.routeStillOnPage() ||
      readPublicQueryForState(props.state).invalidWorld === null ||
      !recoveryOptions().some((option) => option.value === value)
    ) {
      return;
    }
    const keys = ["world", "auction", "lot", "page"];
    if (publicQueryPage(props.state) === "BidLog") keys.push("bidlog_page");
    // 范围、场次和分页随同一次地址提交生效；拒写时保持无可读范围。
    updatePublicQueryPairs(props.state, keys, value === "all" ? [] : [["world", value]], "push");
  };
  return (
    <>
      <div class="public-auction-group-toolbar public-world-toolbar">
        <Show when={props.state().invalidWorld} fallback={<WorldTabs />}>
          <nav class="public-auction-filter public-world-filter" aria-label={t("world-label")}>
            <span class="public-auction-filter__label">{t("world-label")}</span>
            <For each={recoveryOptions()} keyed={(option) => option.value}>
              {(option) => (
                <button
                  type="button"
                  class="public-auction-filter__tab"
                  aria-describedby={WORLD_ERROR_ID}
                  onClick={() => recover(option().value)}
                >
                  {option().value === "all"
                    ? t("auction-history-world-all")
                    : worldLabel(option().value)}
                </button>
              )}
            </For>
          </nav>
        </Show>
      </div>
      <Show when={props.state().invalidWorld}>
        {(values) => (
          <p id={WORLD_ERROR_ID} class="public-auction-empty__title" role="alert">
            {t(values().length > 1 ? "filter-error-duplicate" : "world-invalid-link")}
            <bdi>
              {" "}
              {values()
                .map((value) => JSON.stringify(value))
                .join(" / ")}
            </bdi>
          </p>
        )}
      </Show>
    </>
  );
}
