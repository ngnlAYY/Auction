// 自动周排期与赛季早鸟期参数编辑控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import {
  type RuntimeConfigForm,
  runtimeConfigFormInput,
  runtimeConfigFormSetInput,
} from "../../model";
import { ListField, NumberField, RuntimeGroup, SelectField, SwitchField, TextField } from "..";
import { ManagedSection } from ".";

/** 自动周排期分组及其赛季早鸟期参数。 */
export function ScheduleFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <ManagedSection
      form={props.form}
      disabled={props.disabled}
      path="operations.schedule.managed"
      title={t("admin-runtime-op-section-schedule")}
      note={t("admin-runtime-op-note-schedule")}
      onDirty={props.onDirty}
    >
      <div class="admin-runtime-field-grid">
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.enabled"
          label={t("admin-runtime-op-enabled")}
          onDirty={props.onDirty}
        />
        <TextField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.timezone"
          label={t("admin-runtime-op-timezone")}
          onDirty={props.onDirty}
        />
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.preview_day"
          label={t("admin-runtime-op-preview-day")}
          options={weekdayOptions()}
          onDirty={props.onDirty}
        />
        <TimeField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.preview_time"
          label={t("admin-runtime-op-preview-time")}
          onDirty={props.onDirty}
        />
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.open_day"
          label={t("admin-runtime-op-open-day")}
          options={weekdayOptions()}
          onDirty={props.onDirty}
        />
        <TimeField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.open_time"
          label={t("admin-runtime-op-open-time")}
          onDirty={props.onDirty}
        />
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.nominal_end_day"
          label={t("admin-runtime-op-nominal-end-day")}
          options={weekdayOptions()}
          onDirty={props.onDirty}
        />
        <TimeField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.nominal_end_time"
          label={t("admin-runtime-op-nominal-end-time")}
          onDirty={props.onDirty}
        />
        <ListField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.worlds"
          label={t("admin-runtime-op-worlds")}
          onDirty={props.onDirty}
        />
      </div>
      <EarlyBirdFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
    </ManagedSection>
  );
}

/** 赛季早鸟期：赛季前场次与窗口内场次替代重叠的常规周场、使用单独品质并在赛季前暂停收件。 */
function EarlyBirdFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <RuntimeGroup title={t("admin-runtime-op-early-bird")}>
      <p class="admin-runtime-section-note">{t("admin-runtime-op-early-bird-note")}</p>
      <div class="admin-runtime-field-grid">
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.enabled"
          label={t("admin-runtime-op-enabled")}
          onDirty={props.onDirty}
        />
        <TextField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.timezone"
          label={t("admin-runtime-op-timezone")}
          onDirty={props.onDirty}
        />
        <ListField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.worlds"
          label={t("admin-runtime-op-worlds")}
          onDirty={props.onDirty}
        />
        <ListField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.season_starts"
          label={t("admin-runtime-op-early-bird-season-starts")}
          onDirty={props.onDirty}
        />
        <TimeField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.season_start_time"
          label={t("admin-runtime-op-early-bird-season-start-time")}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.window_days"
          label={t("admin-runtime-op-early-bird-window-days")}
          minimum="1"
          maximum="31"
          onDirty={props.onDirty}
        />
        <ListField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.session_days"
          label={t("admin-runtime-op-early-bird-session-days")}
          onDirty={props.onDirty}
        />
        <TimeField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.open_time"
          label={t("admin-runtime-op-open-time")}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.preview_lead_minutes"
          label={t("admin-runtime-op-early-bird-preview-lead")}
          minimum="120"
          maximum="10080"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.live_minutes"
          label={t("admin-runtime-op-early-bird-live-minutes")}
          minimum="1"
          maximum="10080"
          onDirty={props.onDirty}
        />
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.pre_season_session"
          label={t("admin-runtime-op-early-bird-pre-season")}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.pre_season_days_before"
          label={t("admin-runtime-op-early-bird-pre-season-days")}
          minimum="1"
          maximum="31"
          onDirty={props.onDirty}
        />
        <ListField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.qualities"
          label={t("admin-runtime-op-early-bird-qualities")}
          onDirty={props.onDirty}
        />
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.schedule.early_bird.pause_intake_before_season"
          label={t("admin-runtime-op-early-bird-pause-intake")}
          onDirty={props.onDirty}
        />
      </div>
    </RuntimeGroup>
  );
}

function weekdayOptions(): readonly (readonly [value: string, label: string])[] {
  return [
    ["Monday", t("admin-runtime-op-monday")],
    ["Tuesday", t("admin-runtime-op-tuesday")],
    ["Wednesday", t("admin-runtime-op-wednesday")],
    ["Thursday", t("admin-runtime-op-thursday")],
    ["Friday", t("admin-runtime-op-friday")],
    ["Saturday", t("admin-runtime-op-saturday")],
    ["Sunday", t("admin-runtime-op-sunday")],
  ];
}

function TimeField(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly label: string;
  readonly onDirty: () => void;
}): JSX.Element {
  const value = createValueMemo(() => runtimeConfigFormInput(props.form(), props.path));
  return (
    <label class="admin-runtime-field">
      <span>{props.label}</span>
      <input
        type="time"
        step="60"
        disabled={props.disabled}
        value={value()}
        name={props.path}
        autocomplete="off"
        onInput={(event) => {
          const next = event.currentTarget.value;
          props.form.update((current) => runtimeConfigFormSetInput(current, props.path, next));
          props.onDirty();
        }}
      />
    </label>
  );
}
