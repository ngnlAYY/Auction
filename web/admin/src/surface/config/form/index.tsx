// 运行时配置的分组表单控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { adminWorldLabel } from "../../presentation";
import { FieldLiveRegion, useFieldAnnouncer } from "../field-announcement";
import type { RuntimeConfigForm } from "../model";
import { EquipmentColorRulesField } from "./equipment-colors";
import {
  ListField,
  NumberField,
  RuntimeGroup,
  RuntimeSection,
  SwitchField,
  TextField,
} from "./fields";
import { ListenerSection } from "./listener";
import { MailboxesSection } from "./mailboxes";
import { FieldAnnouncerContext } from "./number";
import { OperationalConfigFields } from "./operations";

/** 运行时配置的全部分组；字段值来自草稿，禁用状态由页面统一给出。 */
export function RuntimeConfigFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  const announcer = useFieldAnnouncer();

  return (
    <FieldAnnouncerContext value={announcer}>
      <div class="admin-runtime-fields">
        <RuntimeSection title={t("admin-runtime-section-auction")} open={false}>
          <div class="admin-runtime-field-grid">
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="allow_manual_early_open"
              label={t("admin-runtime-manual-open")}
              onDirty={props.onDirty}
            />
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="allow_manual_early_settlement"
              label={t("admin-runtime-manual-settlement")}
              onDirty={props.onDirty}
            />
            <ListField
              form={props.form}
              disabled={props.disabled}
              path="bot_players"
              label={t("admin-runtime-bot-players")}
              onDirty={props.onDirty}
            />
          </div>
        </RuntimeSection>

        <RuntimeSection title={t("admin-runtime-section-worlds")} open={false}>
          <div class="admin-runtime-world-grid">
            <RuntimeGroup title={adminWorldLabel("main")}>
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="worlds.main.enabled"
                label={t("admin-runtime-world-enabled")}
                onDirty={props.onDirty}
              />
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="worlds.main.sender_allowlist_disabled"
                label={t("admin-runtime-world-all-senders")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="worlds.main.sender_allowlist"
                label={t("admin-runtime-world-senders")}
                onDirty={props.onDirty}
              />
            </RuntimeGroup>
            <RuntimeGroup title={adminWorldLabel("isekai")}>
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="worlds.isekai.enabled"
                label={t("admin-runtime-world-enabled")}
                onDirty={props.onDirty}
              />
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="worlds.isekai.sender_allowlist_disabled"
                label={t("admin-runtime-world-all-senders")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="worlds.isekai.sender_allowlist"
                label={t("admin-runtime-world-senders")}
                onDirty={props.onDirty}
              />
            </RuntimeGroup>
          </div>
        </RuntimeSection>

        <OperationalConfigFields
          form={props.form}
          disabled={props.disabled}
          onDirty={props.onDirty}
        />

        <RuntimeSection title={t("admin-runtime-section-equipment")} open={false}>
          <RuntimeGroup title={t("admin-runtime-equipment-filter-title")}>
            <p class="admin-runtime-section-note">{t("admin-runtime-quality-gate-note")}</p>
            <div class="admin-runtime-field-grid">
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.enabled"
                label={t("admin-runtime-filter-enabled")}
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.level.min"
                label={t("admin-runtime-level-min")}
                minimum="0"
                maximum="1000"
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.level.max"
                label={t("admin-runtime-level-max")}
                minimum="0"
                maximum="1000"
                onDirty={props.onDirty}
              />
            </div>
            <div class="admin-runtime-field-grid admin-runtime-field-grid--lists">
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.qualities"
                label={t("admin-runtime-filter-qualities")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.prefixes"
                label={t("admin-runtime-filter-prefixes")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.suffixes"
                label={t("admin-runtime-filter-suffixes")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.categories"
                label={t("admin-runtime-filter-categories")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_filter.equipment_types"
                label={t("admin-runtime-filter-types")}
                onDirty={props.onDirty}
              />
            </div>
          </RuntimeGroup>
          <RuntimeGroup title={t("admin-runtime-equipment-colors-title")}>
            <EquipmentColorRulesField
              form={props.form}
              disabled={props.disabled}
              onDirty={props.onDirty}
            />
          </RuntimeGroup>
          <RuntimeGroup title={t("admin-runtime-equipment-source-title")}>
            <div class="admin-runtime-field-grid">
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.allowlist_enabled"
                label={t("admin-runtime-source-allowlist")}
                onDirty={props.onDirty}
              />
              <SwitchField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.require_tradeable"
                label={t("admin-runtime-source-tradeable")}
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.maximum_response_bytes"
                label={t("admin-runtime-source-max-bytes")}
                minimum="65536"
                maximum="8388608"
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.minimum_request_interval_millis"
                label={t("admin-runtime-source-min-interval")}
                minimum="1"
                maximum="60000"
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.request_timeout_seconds"
                label={t("admin-runtime-source-timeout")}
                minimum="1"
                maximum="30"
                onDirty={props.onDirty}
              />
            </div>
            <div class="admin-runtime-field-grid admin-runtime-field-grid--lists">
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.allowed_hosts"
                label={t("admin-runtime-source-hosts")}
                onDirty={props.onDirty}
              />
              <ListField
                form={props.form}
                disabled={props.disabled}
                path="equipment_source.allowed_path_prefixes"
                label={t("admin-runtime-source-paths")}
                onDirty={props.onDirty}
              />
            </div>
          </RuntimeGroup>
        </RuntimeSection>

        <MailboxesSection form={props.form} disabled={props.disabled} onDirty={props.onDirty} />

        <ListenerSection form={props.form} disabled={props.disabled} onDirty={props.onDirty} />

        <RuntimeSection title={t("admin-runtime-section-security")} open={false}>
          <p class="admin-runtime-section-warning">{t("admin-runtime-security-warning")}</p>
          <div class="admin-runtime-field-grid">
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="security.allowed_origins_enabled"
              label={t("admin-runtime-security-origins-enabled")}
              onDirty={props.onDirty}
            />
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="security.cookie_secure"
              label={t("admin-runtime-security-cookie-secure")}
              onDirty={props.onDirty}
            />
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="security.trusted_unix_proxy"
              label={t("admin-runtime-security-trusted-unix")}
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="security.login_rate_limit"
              label={t("admin-runtime-security-login-limit")}
              minimum="1"
              maximum="10000"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="security.mutation_rate_limit"
              label={t("admin-runtime-security-mutation-limit")}
              minimum="1"
              maximum="10000"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="security.rate_limit_window_seconds"
              label={t("admin-runtime-security-window")}
              minimum="1"
              maximum="3600"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="security.request_timeout_seconds"
              label={t("admin-runtime-security-timeout")}
              minimum="1"
              maximum="120"
              onDirty={props.onDirty}
            />
          </div>
          <div class="admin-runtime-field-grid admin-runtime-field-grid--lists">
            <ListField
              form={props.form}
              disabled={props.disabled}
              path="security.allowed_origins"
              label={t("admin-runtime-security-origins")}
              onDirty={props.onDirty}
            />
            <ListField
              form={props.form}
              disabled={props.disabled}
              path="security.trusted_proxy_cidrs"
              label={t("admin-runtime-security-proxy-cidrs")}
              onDirty={props.onDirty}
            />
          </div>
          <RuntimeGroup title={t("admin-runtime-session-title")}>
            <div class="admin-runtime-field-grid">
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="session.idle_ttl_seconds"
                label={t("admin-runtime-session-idle")}
                minimum="60"
                maximum="86400"
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="session.absolute_ttl_seconds"
                label={t("admin-runtime-session-absolute")}
                minimum="60"
                maximum="2592000"
                onDirty={props.onDirty}
              />
              <NumberField
                form={props.form}
                disabled={props.disabled}
                path="session.max_public_sessions"
                label={t("admin-runtime-session-maximum")}
                minimum="1"
                maximum="100"
                onDirty={props.onDirty}
              />
            </div>
          </RuntimeGroup>
        </RuntimeSection>

        <RuntimeSection title={t("admin-runtime-section-observability")} open={false}>
          <div class="admin-runtime-field-grid">
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="metrics.enabled"
              label={t("admin-runtime-metrics-enabled")}
              onDirty={props.onDirty}
            />
            <SwitchField
              form={props.form}
              disabled={props.disabled}
              path="tracing.enabled"
              label={t("admin-runtime-tracing-enabled")}
              onDirty={props.onDirty}
            />
            <TextField
              form={props.form}
              disabled={props.disabled}
              path="tracing.endpoint"
              label={t("admin-runtime-tracing-endpoint")}
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.batch_timeout_seconds"
              label={t("admin-runtime-tracing-batch-timeout")}
              minimum="1"
              maximum="30"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.export_timeout_seconds"
              label={t("admin-runtime-tracing-export-timeout")}
              minimum="1"
              maximum="30"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.max_export_batch_size"
              label={t("admin-runtime-tracing-batch-size")}
              minimum="1"
              maximum="512"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.queue_capacity"
              label={t("admin-runtime-tracing-queue")}
              minimum="1"
              maximum="4096"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.sample_ratio_ppm"
              label={t("admin-runtime-tracing-sample")}
              minimum="0"
              maximum="1000000"
              onDirty={props.onDirty}
            />
            <NumberField
              form={props.form}
              disabled={props.disabled}
              path="tracing.shutdown_timeout_seconds"
              label={t("admin-runtime-tracing-shutdown")}
              minimum="1"
              maximum="30"
              onDirty={props.onDirty}
            />
          </div>
        </RuntimeSection>

        <RuntimeSection title={t("admin-runtime-section-worker")} open={false}>
          <div class="admin-runtime-field-grid">
            <TextField
              form={props.form}
              disabled={props.disabled}
              path="worker_control.socket_path"
              label={t("admin-runtime-worker-socket")}
              onDirty={props.onDirty}
            />
          </div>
        </RuntimeSection>
      </div>
      <FieldLiveRegion announcer={announcer} />
    </FieldAnnouncerContext>
  );
}

export {
  ListField,
  NumberField,
  RuntimeGroup,
  RuntimeSection,
  SelectField,
  SwitchField,
  TextField,
} from "./fields";
