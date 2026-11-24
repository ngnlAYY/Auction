// 登记表单的安全装备标识、本地金额校验与预计报价。

import type {
  ConsignmentStatus,
  ListingMode,
  ListingWorld,
} from "@auction/contracts/dto/public/consignment";
import { LISTING_PRICE_MAX } from "@auction/contracts/dto/public/consignment/scalar";
import { utf8ByteLength } from "@auction/web-shared/text";

/** 仅供展示与重送核对的安全装备标识。 */
export interface RegistrationEquipment {
  readonly world_id: ListingWorld;
  readonly eid: string;
}

/** 识别合同中的装备链接形态，只返回世界和编号。合法性由服务端裁定。 */
export function registrationEquipment(raw: string): RegistrationEquipment | null {
  if (utf8ByteLength(raw) > 2048) return null;
  const text = raw.replace(/[.,;!?。，；！？]+$/u, "");
  const url = text.startsWith("<") && text.endsWith(">") ? text.slice(1, -1) : text;
  // 按既有游戏链接的规范主机与路径提取，不保留 key，也不尝试访问链接。
  const match =
    /^https:\/\/hentaiverse\.org\/(isekai\/)?equip\/([1-9][0-9]{0,31})\/[0-9a-fA-F]{10}$/u.exec(
      url,
    );
  const eid = match?.[2];
  return eid === undefined || match?.[0] !== url
    ? null
    : { world_id: match[1] === undefined ? "main" : "isekai", eid };
}

/** 不变量 6：表单只接受规范正十进制整数，范围由能力和契约共同限制。 */
export function listingPrice(text: string, min = 1, max = LISTING_PRICE_MAX): number | null {
  if (!/^[1-9][0-9]{0,9}$/u.test(text) || /[^0-9]/u.test(text)) return null;
  const price = Number(text);
  return price >= min && price <= max ? price : null;
}

/** 不变量 10：报价仅供确认，卖家已有挂单使用服务端的 estimated 字段。 */
export function estimatedQuote(price: number, bps: number): { fee: number; net: number } {
  const fee = Math.floor((price * bps) / 10_000);
  return { fee, net: price - fee };
}

/** 非寄售费率固定为零，寄售从本次能力快照读取。 */
export function registrationFee(mode: ListingMode, status: ConsignmentStatus): number {
  return mode === "direct" ? 0 : status.fee_bps;
}
