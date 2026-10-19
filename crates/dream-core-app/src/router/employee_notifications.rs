//! Composition adapter: employee domain results use the owner's existing bots.
use dream_core_api_types::CronPushTarget;
use dream_core_channel::{
    ChannelRouterState,
    types::{OutgoingMessageType, UnifiedOutgoingMessage},
};
use std::{path::PathBuf, sync::Arc};

pub(super) struct ChannelEmployeeNotifier(pub ChannelRouterState);

#[async_trait::async_trait]
impl dream_domain_employee::notifications::EmployeeResultNotifier for ChannelEmployeeNotifier {
    async fn validate(&self, owner: &str, target: &CronPushTarget) -> Result<(), String> {
        let plugins = self
            .0
            .manager
            .get_plugin_status(owner)
            .await
            .map_err(|e| e.to_string())?;
        let users = self.0.repo.get_all_users(owner).await.map_err(|e| e.to_string())?;
        let sessions = self.0.repo.get_all_sessions(owner).await.map_err(|e| e.to_string())?;
        let targets = dream_core_channel::push_targets::build_push_targets(plugins, &users, &sessions);
        if targets.iter().any(|bot| {
            bot.plugin_id == target.plugin_id
                && bot.connected
                && ["weixin", "wecom", "lark", "dingtalk", "telegram"].contains(&bot.plugin_type.as_str())
                && bot.chats.iter().any(|chat| chat.chat_id == target.chat_id)
        }) {
            Ok(())
        } else {
            Err("The selected bot or recipient session is unavailable for this user".into())
        }
    }

    async fn send(&self, owner: &str, target: &CronPushTarget, text: &str, files: &[PathBuf]) -> Result<(), String> {
        self.validate(owner, target).await?;
        let message = UnifiedOutgoingMessage {
            message_type: OutgoingMessageType::Text,
            text: Some(text.to_owned()),
            parse_mode: None,
            buttons: Some(vec![]),
            keyboard: None,
            image_url: None,
            file_url: None,
            file_name: None,
            media_actions: None,
            reply_to_message_id: None,
            silent: None,
        };
        self.0
            .manager
            .send_message(owner, &target.plugin_id, &target.chat_id, message.clone())
            .await
            .map_err(|e| e.to_string())?;
        for path in files {
            let mut attachment = message.clone();
            attachment.message_type = OutgoingMessageType::File;
            attachment.text = None;
            attachment.file_url = Some(path.to_string_lossy().into_owned());
            attachment.file_name = path.file_name().map(|name| name.to_string_lossy().into_owned());
            self.0
                .manager
                .send_message(owner, &target.plugin_id, &target.chat_id, attachment)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

pub(super) fn bind(service: &dream_domain_employee::EmployeeService, channel: &ChannelRouterState) {
    service.set_result_notifier(Arc::new(ChannelEmployeeNotifier(channel.clone())));
}
