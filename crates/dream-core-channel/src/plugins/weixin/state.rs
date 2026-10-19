use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{error::ChannelError, plugin_state::PluginStateStore};

use super::types::WeixinRawMessage;

/// Both fields are credentials and must only be stored through the encrypted store.
#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct WeixinState {
    #[serde(default)]
    pub context_tokens: HashMap<String, String>,
    #[serde(default)]
    pub cursor: String,
}

impl WeixinState {
    pub async fn restore(store: Option<&dyn PluginStateStore>, account: &str) -> Result<Self, ChannelError> {
        let Some(store) = store else {
            return Ok(Self::default());
        };
        let Some(value) = store.load(account).await? else {
            return Ok(Self::default());
        };
        serde_json::from_str(&value)
            .map_err(|_| ChannelError::InvalidConfig("Invalid encrypted WeChat runtime state".into()))
    }

    /// Persist context before dispatch, then call again with the new cursor after dispatch.
    /// A failed write retains the old cursor so the batch can be fetched again.
    pub async fn advance(
        &mut self,
        store: Option<&dyn PluginStateStore>,
        account: &str,
        cursor: Option<&str>,
        messages: &[WeixinRawMessage],
    ) -> Result<(), ChannelError> {
        let mut next = Self {
            context_tokens: self.context_tokens.clone(),
            cursor: cursor
                .filter(|cursor| !cursor.is_empty())
                .unwrap_or(&self.cursor)
                .to_owned(),
        };
        for message in messages {
            if let (Some(chat), Some(token)) = (&message.from_user_id, &message.context_token)
                && !chat.is_empty()
                && !token.is_empty()
            {
                next.context_tokens.insert(chat.clone(), token.clone());
            }
        }
        if next == *self {
            return Ok(());
        }
        if let Some(store) = store {
            store.save(account, &serde_json::to_string(&next)?).await?;
        }
        *self = next;
        Ok(())
    }
}
