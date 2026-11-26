// 登记页的模式选择与能力提示，账户登记与免登录登记共用。

import type { ConsignmentStatus, ListingMode } from "@auction/contracts/dto/public/consignment";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Switch } from "solid-js";

import type { ConsignmentCapability } from "./capability";
import { listingModeKey } from "./labels";
import { ConsignmentEmpty } from "./read-view";

/** 模式只能从本次能力快照列出的登记模式中选择。 */
export function ModeChoice(props: {
  readonly status: ConsignmentStatus;
  readonly mode: ListingMode | null;
  readonly onChange: (mode: ListingMode | null) => void;
}): JSX.Element {
  return (
    <label>
      {t("consignment-mode")}
      <select
        value={props.mode ?? ""}
        onChange={(event) => {
          const mode = props.status.registration_modes.find(
            (choice) => choice === event.currentTarget.value,
          );
          props.onChange(mode ?? null);
        }}
      >
        <option value="" disabled>
          {t("consignment-mode")}
        </option>
        <For each={props.status.registration_modes} keyed={false}>
          {(mode) => <option value={mode()}>{t(listingModeKey(mode()))}</option>}
        </For>
      </select>
    </label>
  );
}

/** 能力未就绪时的加载、停用与不可用提示。 */
export function CapabilityNotice(props: {
  readonly capability: ConsignmentCapability | undefined;
  readonly refresh: () => void;
}): JSX.Element {
  return (
    <Switch>
      <Match when={props.capability === undefined}>
        <AsyncState kind="loading" title={t("data-loading")} detail="" />
      </Match>
      <Match when={props.capability?.kind === "disabled"}>
        <ConsignmentEmpty title={t("consignment-disabled")} />
      </Match>
      <Match when={props.capability?.kind === "failed" || props.capability?.kind === "read-only"}>
        <AsyncState
          kind="error"
          title={t("consignment-unavailable")}
          detail=""
          retryLabel={t("data-retry")}
          onRetry={props.refresh}
        />
      </Match>
    </Switch>
  );
}
