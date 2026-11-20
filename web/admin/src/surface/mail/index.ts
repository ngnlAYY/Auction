// Admin 邮件轮询状态与入站审查页面的装配。

/** 邮箱实体前缀；仪表盘和邮件页的暂停、恢复按同一邮箱互斥。 */
export const MAILBOX_ENTITY_PREFIX = "mailbox:";

/** 邮箱命令实体标识；仪表盘与邮件页必须使用同一映射。 */
export function mailboxCommandEntity(mailboxKey: string): string {
  return `${MAILBOX_ENTITY_PREFIX}${mailboxKey}`;
}
