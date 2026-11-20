// 出站通知模板 DTO；数据库修订事实在 HTTP 边界显式映射。

import * as v from "valibot";
import { integer, optionalNullable, strictStruct } from "../../wire";
import { timestamp } from "../../wire/timestamp";

/** 一个模板键的当前状态。 */
export const AdminMailTemplateDtoSchema = strictStruct({
  /** 闭集模板键。 */
  key: v.string(),
  /** 编辑框里的正文：运营保存过的正文，没有或已恢复默认时是内置默认正文。 */
  body: v.string(),
  /** 程序内置的默认正文，供对照与恢复。 */
  default_body: v.string(),
  /** 最新修订是否为运营保存的正文；`false` 表示正在使用内置默认正文。 */
  customized: v.boolean(),
  /** 运营保存的正文是否满足现行占位符规则；不满足时实际发信使用内置默认正文。 */
  usable: v.boolean(),
  /** 最新数据库版本；从未修订过为零，保存与恢复默认都要带上它。 */
  revision: integer,
  /** 最新修订的操作者；从未修订过为空。 */
  updated_by: optionalNullable(v.string()),
  /** 最新修订的数据库接收时间；从未修订过为空。 */
  updated_at: optionalNullable(timestamp),
  /** 正文必须包含的占位符（不含花括号）。 */
  required_placeholders: v.pipe(v.array(v.string()), v.readonly()),
  /** 正文允许使用的全部占位符（不含花括号）。 */
  allowed_placeholders: v.pipe(v.array(v.string()), v.readonly()),
});

export type AdminMailTemplateDto = v.InferOutput<typeof AdminMailTemplateDtoSchema>;

/** 全部模板，顺序固定。 */
export const AdminMailTemplateListResponseSchema = strictStruct({
  /** 每个闭集模板键一项。 */
  templates: v.pipe(v.array(AdminMailTemplateDtoSchema), v.readonly()),
});

export type AdminMailTemplateListResponse = v.InferOutput<
  typeof AdminMailTemplateListResponseSchema
>;

/** 模板列表中的 `revision` 决定编辑与提交能否继续；解析完整响应原文时必须传入，
 * 拒绝重复键与小数、指数、`-0` 等非整数词法。 */
export const ADMIN_MAIL_TEMPLATE_LIST_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** 保存或恢复默认一个模板的稳定命令。
 *
 * 结果未知时必须连同 `command_id`、`expected_revision` 与 `body` 原样重试。 */
export interface AdminMailTemplateUpdateRequest {
  /** 稳定命令 ID（UUID）。 */
  command_id: string;
  /** 读取到的最新版本；从未修订过为零。 */
  expected_revision: number;
  /** 新正文；`null` 表示恢复内置默认正文。 */
  body: string | null;
}

/** 数据库已接受的修订；各角色约 5 秒内刷新生效。 */
export const AdminMailTemplateUpdateResponseSchema = strictStruct({
  /** 修订后的模板状态。 */
  template: AdminMailTemplateDtoSchema,
});

export type AdminMailTemplateUpdateResponse = v.InferOutput<
  typeof AdminMailTemplateUpdateResponseSchema
>;

/** 模板保存/恢复回执用于判定写入是否被接受；解析原文时必须传入。
 * 回执里的具型数字只有修订号，因此可以整份拒绝小数、指数与 `-0`。 */
export const ADMIN_MAIL_TEMPLATE_UPDATE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;
