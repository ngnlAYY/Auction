// 运营挂单共用的解析进度、快照选取与安全错误文案。

import type { EquipmentView, LookupProgress } from "@auction/contracts/dto/public/consignment";
import type { ListingViewV2 } from "@auction/contracts/dto/public/consignment/listing-v2";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { adminApiErrorMessage } from "../presentation";
import { listingLookupReasonKey, listingWriteErrorKey } from "./labels";

/** 只从服务端指定带快照的视图读取装备信息。 */
export function listingEquipment(view: ListingViewV2): EquipmentView | null {
  return "equipment" in view ? view.equipment : null;
}

/** 解析中的次数、下次尝试、暂停及最近的安全失败分类。 */
export function ListingProgress(props: { readonly progress: LookupProgress }): JSX.Element {
  return (
    <div class="admin-consignment-progress">
      <span>
        {t("consignment-attempts")} {props.progress.attempts}/{props.progress.max_attempts}
      </span>
      <Show when={props.progress.next_attempt_at}>
        {(next) => (
          <span>
            {t("consignment-next-attempt")} <Time value={next()} />
          </span>
        )}
      </Show>
      <Show when={props.progress.paused}>
        <span>{t("consignment-paused")}</span>
      </Show>
      <Show when={props.progress.last_reason}>
        {(reason) => <span>{t(listingLookupReasonKey(reason()))}</span>}
      </Show>
    </div>
  );
}

/** 寄售业务冲突用专用文案，其余错误用管理端的安全提示。 */
export function consignmentErrorMessage(code: string): string {
  const key = listingWriteErrorKey(code);
  if (key !== null) return t(key);
  if (code === "listing_not_found") return adminApiErrorMessage("not_found");
  if (code === "consignment_unavailable") return adminApiErrorMessage("storage_unavailable");
  return adminApiErrorMessage(code);
}
