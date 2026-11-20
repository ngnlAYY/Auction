// 运维告警参数编辑控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { adminWorldLabel } from "../../../presentation";
import type { RuntimeConfigForm } from "../../model";
import { NumberField, SelectField, SwitchField, TextField } from "..";
import { ManagedSection } from ".";

/** 运维告警分组。 */
export function AlertsFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <ManagedSection
      form={props.form}
      disabled={props.disabled}
      path="operations.alerts.managed"
      title={t("admin-runtime-op-section-alerts")}
      note={t("admin-runtime-op-note-alerts")}
      onDirty={props.onDirty}
    >
      <div class="admin-runtime-field-grid">
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.enabled"
          label={t("admin-runtime-op-enabled")}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.scan_interval_seconds"
          label={t("admin-runtime-op-scan-interval-seconds")}
          minimum="1"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.jitter_seconds"
          label={t("admin-runtime-op-jitter-seconds")}
          minimum="0"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.source_timeout_seconds"
          label={t("admin-runtime-op-source-timeout-seconds")}
          minimum="1"
          maximum="30"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.batch_window_seconds"
          label={t("admin-runtime-op-batch-window-seconds")}
          minimum="1"
          maximum="3600"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.cooldown_seconds"
          label={t("admin-runtime-op-cooldown-seconds")}
          minimum="1"
          maximum="604800"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.claim_lease_seconds"
          label={t("admin-runtime-op-claim-lease-seconds")}
          minimum="1"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.retry_delay_seconds"
          label={t("admin-runtime-op-retry-delay-seconds")}
          minimum="1"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.max_signals_per_scan"
          label={t("admin-runtime-op-max-signals-per-scan")}
          minimum="1"
          maximum="64"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.max_batch_items"
          label={t("admin-runtime-op-max-batch-items")}
          minimum="1"
          maximum="64"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.max_body_bytes"
          label={t("admin-runtime-op-max-body-bytes")}
          minimum="256"
          maximum="16384"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.delivery_pending_count_threshold"
          label={t("admin-runtime-op-delivery-pending-count-threshold")}
          minimum="1"
          maximum="9007199254740991"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.delivery_pending_age_seconds"
          label={t("admin-runtime-op-delivery-pending-age-seconds")}
          minimum="1"
          maximum="2592000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.delivery_manual_reconcile_count_threshold"
          label={t("admin-runtime-op-delivery-manual-reconcile-count-threshold")}
          minimum="1"
          maximum="9007199254740991"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.fulfillment_backlog_count_threshold"
          label={t("admin-runtime-op-fulfillment-backlog-count-threshold")}
          minimum="1"
          maximum="9007199254740991"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.fulfillment_backlog_age_seconds"
          label={t("admin-runtime-op-fulfillment-backlog-age-seconds")}
          minimum="1"
          maximum="2592000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.fulfillment_collection_overdue_age_seconds"
          label={t("admin-runtime-op-fulfillment-collection-overdue-age-seconds")}
          minimum="1"
          maximum="2592000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.recovery_overdue_count_threshold"
          label={t("admin-runtime-op-recovery-overdue-count-threshold")}
          minimum="1"
          maximum="9007199254740991"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.recovery_overdue_age_seconds"
          label={t("admin-runtime-op-recovery-overdue-age-seconds")}
          minimum="1"
          maximum="2592000"
          onDirty={props.onDirty}
        />
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.world_id"
          label={t("admin-runtime-op-world-id")}
          options={[
            ["main", adminWorldLabel("main")],
            ["isekai", adminWorldLabel("isekai")],
          ]}
          onDirty={props.onDirty}
        />
        <TextField
          form={props.form}
          disabled={props.disabled}
          path="operations.alerts.recipient"
          label={t("admin-runtime-op-recipient")}
          onDirty={props.onDirty}
        />
      </div>
    </ManagedSection>
  );
}
