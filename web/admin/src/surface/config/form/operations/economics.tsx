// 后续新拍卖经济参数编辑控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { RuntimeConfigForm } from "../../model";
import { NumberField, SelectField, SwitchField } from "..";
import { ManagedSection } from ".";

/** 新拍卖经济参数分组。 */
export function EconomicsFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <ManagedSection
      form={props.form}
      disabled={props.disabled}
      path="operations.economics.managed"
      title={t("admin-runtime-op-section-economics")}
      note={t("admin-runtime-op-note-economics")}
      onDirty={props.onDirty}
    >
      <div class="admin-runtime-field-grid">
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.start_price"
          label={t("admin-runtime-op-start-price")}
          minimum="1000"
          maximum="1000000000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.bid_increment_ppm"
          label={t("admin-runtime-op-bid-increment-ppm")}
          minimum="0"
          maximum="1000000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.soft_close_window_seconds"
          label={t("admin-runtime-op-soft-close-window-seconds")}
          minimum="0"
          maximum="604800"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.soft_close_extension_seconds"
          label={t("admin-runtime-op-soft-close-extension-seconds")}
          minimum="1"
          maximum="604800"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.cancel_cutoff_seconds"
          label={t("admin-runtime-op-cancel-cutoff-seconds")}
          minimum="0"
          maximum="604800"
          onDirty={props.onDirty}
        />
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.proxy_enabled"
          label={t("admin-runtime-op-proxy-enabled")}
          onDirty={props.onDirty}
        />
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.proxy_default_mode"
          label={t("admin-runtime-op-proxy-default-mode")}
          options={[
            ["fast", t("admin-runtime-op-mode-fast")],
            ["slow", t("admin-runtime-op-mode-slow")],
          ]}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.proxy_slow_interval_seconds"
          label={t("admin-runtime-op-proxy-slow-interval-seconds")}
          minimum="1"
          maximum="604800"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.proxy_max_outbid_uses"
          label={t("admin-runtime-op-proxy-max-outbid-uses")}
          minimum="0"
          maximum="1000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.economics.final_phase_lead_seconds"
          label={t("admin-runtime-op-final-phase-lead-seconds")}
          minimum="1"
          maximum="604800"
          onDirty={props.onDirty}
        />
      </div>
    </ManagedSection>
  );
}
