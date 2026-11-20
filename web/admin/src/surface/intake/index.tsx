// Admin 手工装备录入表单。
// 旧版拍卖总览和 lot 草稿页都提供同一份录入能力。表单只负责收集来源与命令参数，
// 具体解析、幂等和权限判断仍由既有 Admin API 与应用层负责。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import { type AdminIntakeResult, addAdminIntake, prepareAdminIntakeOnce } from "../../api/intake";
import { useAdminSessionHost } from "../auth/shell";
import { parseStartPrice } from "../lots/presentation";
import { type CommandDelivery, requirePendingCommands, setIfAlive } from "../pending";
import { adminApiErrorMessage, adminWorldLabel } from "../presentation";
import { FieldError, useFocusField } from "../validation";
import {
  emptyIntakeFieldErrors,
  type IntakeFieldErrors,
  intakeFieldErrorsAny,
  intakeFieldErrorsFirstInvalidId,
  optionalIntakeFieldIssueMessage,
  rustTrim,
  validCanonicalEquipmentJson,
} from "./validation";

/** 展示手工装备录入表单，并在成功后触发所属页面重新加载。 */
export function AdminIntakeForm(props: { readonly worldId: string }): JSX.Element {
  const owner = requirePendingCommands();
  const host = useAdminSessionHost();
  const focusField = useFocusField();
  const entity = createValueMemo(() => `intake:${props.worldId}`);
  const sending = () => owner.isBusy(entity());
  // 控件锁来自页级未决集合；表单卸载不得释放任务，未知状态也不能编辑为新意图。
  const busy = createValueMemo(() => owner.contains(entity()));
  const sourceKind = createCell("url");
  const equipmentUrl = createCell("");
  const equipmentJson = createCell("");
  const startPrice = createCell("");
  const playerId = createCell("");
  // 服务端返回的错误码；输入层校验错误由各字段状态承担，不走整页提示。
  const failure = createCell<string | null>(null);
  const fieldErrors = createCell<IntakeFieldErrors>(emptyIntakeFieldErrors());
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const failureAlive = () => alive;

  const onSubmit = (event: SubmitEvent) => {
    event.preventDefault();
    if (busy()) {
      return;
    }
    const submittedSourceKind = sourceKind();
    const submittedEquipmentUrl = rustTrim(equipmentUrl());
    const submittedEquipmentJson = rustTrim(equipmentJson());
    const submittedPlayerId = rustTrim(playerId());
    // 逐字段校验并定位错误；提交失败聚焦第一个错误字段。
    const errors: IntakeFieldErrors = {
      source: null,
      price: null,
      player: null,
    };
    if (submittedSourceKind === "url") {
      if (submittedEquipmentUrl === "" || utf8ByteLength(submittedEquipmentUrl) > 2048) {
        errors.source = "required";
      }
    } else if (!validCanonicalEquipmentJson(submittedEquipmentJson)) {
      errors.source = "invalid_json";
    }
    const submittedStartPrice = parseStartPrice(startPrice());
    if (submittedStartPrice === null) {
      errors.price = "invalid_price";
    }
    if (submittedPlayerId === "" || utf8ByteLength(submittedPlayerId) > 256) {
      errors.player = "required";
    }
    if (intakeFieldErrorsAny(errors)) {
      fieldErrors.set(errors);
      const fieldId = intakeFieldErrorsFirstInvalidId(errors, submittedSourceKind);
      if (fieldId !== null) {
        focusField(fieldId);
      }
      return;
    }
    // 起拍价校验未过时 errors 非空并在上面返回，这里必有值。
    if (submittedStartPrice === null) {
      return;
    }
    fieldErrors.set(emptyIntakeFieldErrors());
    failure.set(null);
    const worldId = props.worldId;
    // 只冻结已选择来源的参数，另一来源的未提交草稿不进入页宿主。
    const frozenUrl = submittedSourceKind === "url" ? submittedEquipmentUrl : "";
    const frozenJson = submittedSourceKind === "json" ? submittedEquipmentJson : "";
    const frozenPlayerId = submittedPlayerId;
    const startPriceValue = submittedStartPrice;
    const prepare = prepareAdminIntakeOnce(
      submittedSourceKind,
      frozenUrl,
      frozenJson,
      startPriceValue,
      frozenPlayerId,
    );
    owner.submitWithReceipt(
      `intake:${worldId}`,
      t("admin-intake-title", { world: adminWorldLabel(worldId) }),
      async (commandId, signal) => {
        return await addAdminIntake(
          host,
          commandId,
          worldId,
          startPriceValue,
          frozenPlayerId,
          prepare,
          signal,
        );
      },
      (value, sourceAlive): CommandDelivery => {
        if (sourceAlive) {
          setIfAlive(failure, null, failureAlive);
          setIfAlive(fieldErrors, emptyIntakeFieldErrors(), failureAlive);
        }
        // 场次或拍品的重读可以卸载录入表单，已接受的录入由会话交付。
        return {
          kind: "receipt",
          receipt: { kind: "text", text: intakeResultMessage(value) },
        };
      },
      (code) => {
        const errors: IntakeFieldErrors = {
          source: null,
          price: null,
          player: null,
        };
        switch (code) {
          case "seller_blacklisted":
          case "auction_intake_player_blacklisted":
            errors.player = "player_blacklisted";
            break;
          case "equipment_conflict":
            errors.source = "equipment_conflict";
            break;
          default:
            break;
        }
        setIfAlive(fieldErrors, errors, failureAlive);
        setIfAlive(failure, code, failureAlive);
      },
    );
  };

  return (
    <section
      class="admin-auction-intake"
      aria-labelledby="auction-lot-intake-title"
      aria-busy={sending() ? "true" : "false"}
    >
      <h2 id="auction-lot-intake-title">
        {t("admin-intake-title", { world: adminWorldLabel(props.worldId) })}
      </h2>
      <p>{t("admin-intake-description")}</p>
      <form class="admin-auction-intake-form" onSubmit={onSubmit}>
        <label for="auction-intake-source">
          {t("admin-intake-source")}
          <select
            id="auction-intake-source"
            name="source"
            autocomplete="off"
            disabled={busy()}
            value={sourceKind()}
            onChange={(event) => {
              sourceKind.set(event.currentTarget.value);
              failure.set(null);
              fieldErrors.update((current) => ({ ...current, source: null }));
            }}
          >
            <option value="url">{t("admin-intake-source-url")}</option>
            <option value="json">{t("admin-intake-source-json")}</option>
          </select>
        </label>
        <Show
          when={sourceKind() === "url"}
          fallback={
            <div class="admin-auction-intake-json">
              <label for="auction-intake-json">{t("admin-intake-equipment-json")}</label>
              <textarea
                id="auction-intake-json"
                name="equipment_json"
                autocomplete="off"
                autocapitalize="none"
                spellcheck="false"
                aria-describedby={
                  fieldErrors().source !== null
                    ? "auction-intake-json-hint auction-intake-json-error"
                    : "auction-intake-json-hint"
                }
                aria-invalid={fieldErrors().source !== null ? "true" : undefined}
                rows="8"
                maxlength="65536"
                placeholder={t("admin-intake-json-hint")}
                required
                disabled={busy()}
                value={equipmentJson()}
                onInput={(event) => {
                  equipmentJson.set(event.currentTarget.value);
                  failure.set(null);
                  fieldErrors.update((current) => ({ ...current, source: null }));
                }}
              />
              <span id="auction-intake-json-hint">{t("admin-intake-json-hint")}</span>
              <FieldError
                id="auction-intake-json-error"
                message={optionalIntakeFieldIssueMessage(fieldErrors().source)}
              />
            </div>
          }
        >
          <label for="auction-intake-url">
            {t("admin-intake-equipment-url")}
            <input
              id="auction-intake-url"
              name="equipment_url"
              inputmode="url"
              autocapitalize="none"
              spellcheck="false"
              type="url"
              autocomplete="off"
              maxlength="2048"
              required
              disabled={busy()}
              aria-invalid={fieldErrors().source !== null ? "true" : undefined}
              aria-describedby={
                fieldErrors().source !== null ? "auction-intake-url-error" : undefined
              }
              value={equipmentUrl()}
              onInput={(event) => {
                equipmentUrl.set(event.currentTarget.value);
                failure.set(null);
                fieldErrors.update((current) => ({ ...current, source: null }));
              }}
            />
          </label>
          <FieldError
            id="auction-intake-url-error"
            message={optionalIntakeFieldIssueMessage(fieldErrors().source)}
          />
        </Show>
        <label for="auction-intake-price">
          {t("admin-intake-start-price")}
          <input
            id="auction-intake-price"
            name="start_price"
            inputmode="numeric"
            autocomplete="off"
            type="number"
            min="1"
            step="1"
            required
            disabled={busy()}
            aria-invalid={fieldErrors().price !== null ? "true" : "false"}
            aria-describedby={
              fieldErrors().price !== null ? "auction-intake-price-error" : undefined
            }
            value={startPrice()}
            onInput={(event) => {
              startPrice.set(event.currentTarget.value);
              failure.set(null);
              fieldErrors.update((current) => ({ ...current, price: null }));
            }}
          />
        </label>
        <FieldError
          id="auction-intake-price-error"
          message={optionalIntakeFieldIssueMessage(fieldErrors().price)}
        />
        <label for="auction-intake-player">
          {t("admin-intake-player-id")}
          <input
            id="auction-intake-player"
            name="player"
            autocapitalize="none"
            spellcheck="false"
            type="text"
            autocomplete="off"
            maxlength="256"
            required
            disabled={busy()}
            aria-invalid={fieldErrors().player !== null ? "true" : undefined}
            aria-describedby={
              fieldErrors().player !== null ? "auction-intake-player-error" : undefined
            }
            value={playerId()}
            onInput={(event) => {
              playerId.set(event.currentTarget.value);
              failure.set(null);
              fieldErrors.update((current) => ({ ...current, player: null }));
            }}
          />
        </label>
        <FieldError
          id="auction-intake-player-error"
          message={optionalIntakeFieldIssueMessage(fieldErrors().player)}
        />
        <button type="submit" disabled={busy()}>
          {sending() ? t("admin-action-working") : t("admin-intake-submit")}
        </button>
      </form>
      <Show when={failure()}>
        {(code) => (
          <div
            role="alert"
            aria-live="assertive"
            class="admin-panel__note action-feedback action-feedback--error"
          >
            <strong>{t("admin-intake-error")}</strong>
            <br />
            <span>{adminApiErrorMessage(code())}</span>
          </div>
        )}
      </Show>
    </section>
  );
}

function intakeResultMessage(value: AdminIntakeResult): string {
  return t("admin-intake-result", {
    name: value.equipment_name,
    number: value.auction_sequence,
    lot: value.lot_number,
    category: value.equipment_category,
    level: value.equipment_level === null ? t("value-empty") : String(value.equipment_level),
  });
}
