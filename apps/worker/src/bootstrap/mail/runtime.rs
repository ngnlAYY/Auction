//! MoogleMail world gateway、持久化 poll state 和轮询运行时装配。

use super::super::*;

struct MoogleMailWorkerGateway {
    gateway: Arc<MoogleMailGateway>,
    world: MoogleMailWorld,
}

pub(super) async fn build_poll_runtimes(
    mail_config: &auction_config::MoogleMailWorkerConfig,
    world_configs: Vec<auction_config::MoogleMailWorldConfig>,
    client: Arc<MoogleMailClient>,
    credentials: MoogleMailCredentialSet,
    processor: Arc<dyn MailProcessor>,
    store: Arc<DatabaseStore>,
    schedule: Arc<dyn RecurringSchedule>,
) -> anyhow::Result<(Vec<MailPollRuntime>, HashMap<MoogleMailWorld, Arc<MoogleMailGateway>>)> {
    let store_port: Arc<dyn MailPollStore> = store.clone();
    let mut gateways = HashMap::with_capacity(world_configs.len());
    let mut runtimes = Vec::with_capacity(world_configs.len());
    for world_config in world_configs {
        let world = MoogleMailWorld::parse(&world_config.world_id)
            .context("MoogleMail world_id must be main or isekai")?;
        let gateway = Arc::new(
            MoogleMailGateway::new(
                Arc::clone(&client),
                credentials.clone(),
                MoogleMailGatewayConfig {
                    maximum_pages: mail_config.maximum_pages,
                    sender_allowlist: world_config.sender_allowlist,
                    sender_allowlist_disabled: world_config.sender_allowlist_disabled,
                    fetch_budget: std::time::Duration::from_secs(mail_config.fetch_timeout_seconds),
                },
            )
            .map_err(|error| anyhow::Error::msg(error.to_string()))?,
        );
        let now = Utc::now();
        let scheduled_next_at =
            schedule.next_after(now).context("calculate next MoogleMail poll schedule")?;
        store_port
            .ensure_state(
                world_config.mailbox_key.clone(),
                now,
                scheduled_next_at,
                std::time::Duration::from_secs(mail_config.retry_maximum_seconds),
            )
            .await
            .context("initialize or reconcile MoogleMail poll state")?;
        gateways.insert(world, Arc::clone(&gateway));
        let gateway_port: Arc<dyn MailPollGateway> =
            Arc::new(MoogleMailWorkerGateway { gateway, world });
        runtimes.push(MailPollRuntime::new(
            Arc::clone(&store_port),
            gateway_port,
            Some(Arc::clone(&processor)),
            world_config.world_id,
            world_config.mailbox_key,
            Arc::clone(&schedule),
            MailPollRuntimeConfig {
                batch_size: mail_config.batch_size,
                jitter: std::time::Duration::from_secs(mail_config.jitter_seconds),
                lease_duration: std::time::Duration::from_secs(mail_config.lease_duration_seconds),
                renew_interval: std::time::Duration::from_secs(mail_config.renew_interval_seconds),
                fetch_attempts: mail_config.fetch_attempts,
                fetch_timeout: std::time::Duration::from_secs(mail_config.fetch_timeout_seconds),
                process_attempts: mail_config.process_attempts,
                item_timeout: std::time::Duration::from_secs(mail_config.item_timeout_seconds),
                batch_timeout: std::time::Duration::from_secs(mail_config.batch_timeout_seconds),
                retry_base: std::time::Duration::from_secs(mail_config.retry_base_seconds),
                retry_maximum: std::time::Duration::from_secs(mail_config.retry_maximum_seconds),
            },
        )?);
    }
    Ok((runtimes, gateways))
}

impl MailPollGateway for MoogleMailWorkerGateway {
    fn fetch(
        &self,
        request: MailPollFetchRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<MailPollPage, MailPollGatewayError>>
                + Send
                + '_,
        >,
    > {
        let gateway = Arc::clone(&self.gateway);
        let world = self.world;
        Box::pin(async move {
            if request.world_id != world.as_str() {
                return Err(MailPollGatewayError {
                    failure_code: "configuration".to_owned(),
                    safe_detail: "MoogleMail Worker 世界配置与 gateway 不一致".to_owned(),
                    temporary: false,
                });
            }
            let page = gateway
                .fetch_with_deadline(
                    world,
                    request.cursor_value.as_deref(),
                    usize::from(request.limit),
                    request.deadline,
                )
                .await
                .map_err(|error| MailPollGatewayError {
                    failure_code: error.category.as_str().to_owned(),
                    safe_detail: format!("MoogleMail {} 操作失败", error.category),
                    temporary: error.is_temporary(),
                })?;
            let messages = page
                .messages
                .into_iter()
                .map(|message| {
                    super::mapping::map_mooglemail_message(message, request.mailbox_key.clone())
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| MailPollGatewayError {
                    failure_code: "protocol_drift".to_owned(),
                    safe_detail: "MoogleMail 业务 envelope 转换失败".to_owned(),
                    temporary: false,
                })?;
            Ok(MailPollPage {
                mailbox_key: request.mailbox_key,
                world_id: world.as_str().to_owned(),
                next_cursor: page.next_cursor,
                messages,
            })
        })
    }
}
