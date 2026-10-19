use std::time::Duration;

use base64::Engine;
use reqwest::Client;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::constants::{WEIXIN_API_TIMEOUT, WEIXIN_POLL_TIMEOUT};
use crate::error::ChannelError;

use super::types::{
    GetUpdatesRequest, GetUpdatesResponse, ILinkResponse, ITEM_TYPE_TEXT, QrCodeData, QrCodeStatusData,
    SendMessageItem, SendMessageMsg, SendMessageRequest, SendTextItem,
};

/// HTTP client for the WeChat iLink Bot API.
pub(crate) struct WeixinApi {
    client: Client,
    base_url: String,
    bot_token: String,
    wechat_uin: String,
}

impl WeixinApi {
    pub async fn send_file(&self, to: &str, path: &str, name: &str, context: Option<&str>) -> Result<(), ChannelError> {
        use aes::cipher::{BlockEncrypt, KeyInit};
        use md5::{Digest, Md5};
        let raw = tokio::fs::read(path)
            .await
            .map_err(|e| ChannelError::MessageSendFailed(e.to_string()))?;
        if raw.is_empty() || raw.len() > 20 * 1024 * 1024 {
            return Err(ChannelError::InvalidConfig(
                "Attachment must be between 1 byte and 20 MiB".into(),
            ));
        }
        let mut key = [0u8; 16];
        getrandom::getrandom(&mut key)
            .map_err(|_| ChannelError::MessageSendFailed("Cannot generate attachment key".into()))?;
        let hex_key = key.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let file_key = Uuid::new_v4().simple().to_string();
        let pad = 16 - raw.len() % 16;
        let mut encrypted = raw.clone();
        encrypted.resize(raw.len() + pad, pad as u8);
        let cipher = aes::Aes128::new_from_slice(&key)
            .map_err(|_| ChannelError::MessageSendFailed("Invalid attachment key".into()))?;
        for chunk in encrypted.chunks_exact_mut(16) {
            cipher.encrypt_block(chunk.into());
        }
        let upload: serde_json::Value = self.authenticated_post("ilink/bot/getuploadurl", &serde_json::json!({"filekey":file_key,"media_type":3,"to_user_id":to,"rawsize":raw.len(),"rawfilemd5":format!("{:x}", Md5::digest(&raw)),"filesize":encrypted.len(),"no_need_thumb":true,"aeskey":hex_key,"base_info":{}}), WEIXIN_API_TIMEOUT).await?;
        check_send_result(&upload)?;
        let url = if let Some(url) = upload["upload_full_url"].as_str().filter(|value| !value.is_empty()) {
            reqwest::Url::parse(url).map_err(|_| ChannelError::MessageSendFailed("Invalid WeChat upload URL".into()))?
        } else {
            let param = upload["upload_param"]
                .as_str()
                .ok_or_else(|| ChannelError::MessageSendFailed("WeChat returned no upload parameters".into()))?;
            let mut url = reqwest::Url::parse("https://novac2c.cdn.weixin.qq.com/c2c/upload").expect("constant URL");
            url.query_pairs_mut()
                .append_pair("encrypted_query_param", param)
                .append_pair("filekey", &file_key);
            url
        };
        if url.scheme() != "https" {
            return Err(ChannelError::MessageSendFailed(
                "WeChat attachment upload requires HTTPS".into(),
            ));
        }
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .timeout(WEIXIN_API_TIMEOUT)
            .body(encrypted)
            .send()
            .await
            .map_err(|_| ChannelError::MessageSendFailed("WeChat attachment upload failed".into()))?
            .error_for_status()
            .map_err(|_| ChannelError::MessageSendFailed("WeChat attachment upload rejected".into()))?;
        let download = response
            .headers()
            .get("x-encrypted-param")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ChannelError::MessageSendFailed("WeChat returned no attachment receipt".into()))?;
        let body = serde_json::json!({"msg":{"to_user_id":to,"client_id":Uuid::new_v4().to_string(),"message_type":2,"message_state":2,"context_token":context,"item_list":[{"type":4,"file_item":{"media":{"encrypt_query_param":download,"aes_key":base64::engine::general_purpose::STANDARD.encode(hex_key.as_bytes()),"encrypt_type":1},"file_name":name,"len":raw.len().to_string()}}]},"base_info":{}});
        let sent: serde_json::Value = self
            .authenticated_post("ilink/bot/sendmessage", &body, WEIXIN_API_TIMEOUT)
            .await?;
        check_send_result(&sent)
    }
    pub fn new(client: Client, base_url: &str, bot_token: &str) -> Self {
        let base = base_url.trim_end_matches('/');

        let mut uin_bytes = [0u8; 4];
        getrandom::getrandom(&mut uin_bytes).expect("RNG failure");
        // The official connector encodes decimal uint32 text, not the raw bytes.
        let uin_decimal = u32::from_be_bytes(uin_bytes).to_string();
        let wechat_uin = base64::engine::general_purpose::STANDARD.encode(uin_decimal.as_bytes());

        Self {
            client,
            base_url: base.to_string(),
            bot_token: bot_token.to_string(),
            wechat_uin,
        }
    }

    #[cfg(test)]
    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }

    #[cfg(test)]
    pub fn wechat_uin(&self) -> &str {
        &self.wechat_uin
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    async fn authenticated_post<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &impl Serialize,
        timeout: Duration,
    ) -> Result<T, ChannelError> {
        let url = format!("{}/{}", self.base_url, endpoint);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .header("X-WECHAT-UIN", &self.wechat_uin)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|e| ChannelError::PlatformApi(format!("{endpoint} request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ChannelError::PlatformApi(format!("{endpoint} HTTP {status}: {text}")));
        }

        resp.json()
            .await
            .map_err(|e| ChannelError::PlatformApi(format!("{endpoint} parse failed: {e}")))
    }

    async fn ilink_get<T: DeserializeOwned>(&self, endpoint: &str, query: &[(&str, &str)]) -> Result<T, ChannelError> {
        let url = format!("{}/{}", self.base_url, endpoint);

        let resp = self
            .client
            .get(&url)
            .header("iLink-App-ClientVersion", "1")
            .query(query)
            .send()
            .await
            .map_err(|e| ChannelError::PlatformApi(format!("{endpoint} request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ChannelError::PlatformApi(format!("{endpoint} HTTP {status}: {text}")));
        }

        resp.json()
            .await
            .map_err(|e| ChannelError::PlatformApi(format!("{endpoint} parse failed: {e}")))
    }

    // -----------------------------------------------------------------------
    // QR code login
    // -----------------------------------------------------------------------

    /// Fetch a QR code for bot login.
    ///
    /// `GET /ilink/bot/get_bot_qrcode?bot_type=3`
    pub async fn get_bot_qrcode(&self) -> Result<QrCodeData, ChannelError> {
        debug!("Fetching WeChat QR code");

        // Try direct response first, then wrapped
        let result: Result<QrCodeData, _> = self.ilink_get("ilink/bot/get_bot_qrcode", &[("bot_type", "3")]).await;

        match result {
            Ok(data) if data.qrcode.is_some() => Ok(data),
            _ => {
                let wrapped: ILinkResponse<QrCodeData> =
                    self.ilink_get("ilink/bot/get_bot_qrcode", &[("bot_type", "3")]).await?;
                wrapped
                    .data
                    .ok_or_else(|| ChannelError::PlatformApi("get_bot_qrcode returned no data".into()))
            }
        }
    }

    /// Check the status of a QR code scan.
    ///
    /// `GET /ilink/bot/get_qrcode_status?qrcode=<ticket>`
    pub async fn get_qrcode_status(&self, qrcode: &str) -> Result<QrCodeStatusData, ChannelError> {
        // Try direct response first, then wrapped
        let result: Result<QrCodeStatusData, _> = self
            .ilink_get("ilink/bot/get_qrcode_status", &[("qrcode", qrcode)])
            .await;

        match result {
            Ok(data) if data.status.is_some() => Ok(data),
            _ => {
                let wrapped: ILinkResponse<QrCodeStatusData> = self
                    .ilink_get("ilink/bot/get_qrcode_status", &[("qrcode", qrcode)])
                    .await?;
                wrapped
                    .data
                    .ok_or_else(|| ChannelError::PlatformApi("get_qrcode_status returned no data".into()))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Long-polling
    // -----------------------------------------------------------------------

    /// Long-poll for new updates using buffer-based protocol.
    ///
    /// `POST /ilink/bot/getupdates`
    pub async fn get_updates(&self, buf: &str) -> Result<GetUpdatesResponse, ChannelError> {
        let body = GetUpdatesRequest {
            get_updates_buf: buf.to_string(),
            base_info: serde_json::json!({}),
        };

        let timeout = WEIXIN_POLL_TIMEOUT + Duration::from_secs(10);

        self.authenticated_post("ilink/bot/getupdates", &body, timeout).await
    }

    // -----------------------------------------------------------------------
    // Send message
    // -----------------------------------------------------------------------

    /// Send a text message.
    ///
    /// `POST /ilink/bot/sendmessage`
    pub async fn send_message(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> Result<(), ChannelError> {
        debug!(to_user_id, "Sending WeChat message");

        let body = SendMessageRequest {
            msg: SendMessageMsg {
                to_user_id: to_user_id.to_string(),
                client_id: Uuid::new_v4().to_string(),
                message_type: 2,
                message_state: 2,
                item_list: vec![SendMessageItem {
                    item_type: ITEM_TYPE_TEXT,
                    text_item: Some(SendTextItem { text: text.to_string() }),
                }],
                context_token: context_token.map(String::from),
            },
            base_info: serde_json::json!({}),
        };

        let response: serde_json::Value = self
            .authenticated_post("ilink/bot/sendmessage", &body, WEIXIN_API_TIMEOUT)
            .await
            .map_err(|e| {
                warn!(to_user_id, error = %e, "sendmessage failed");
                ChannelError::MessageSendFailed(format!("sendmessage failed: {e}"))
            })?;

        check_send_result(&response)
    }
}

fn check_send_result(value: &serde_json::Value) -> Result<(), ChannelError> {
    if value["ret"].as_i64().is_some_and(|code| code != 0) || value["errcode"].as_i64().is_some_and(|code| code != 0) {
        tracing::warn!(
            ret = value["ret"].as_i64(),
            errcode = value["errcode"].as_i64(),
            "WeChat rejected outgoing message"
        );
        return Err(ChannelError::MessageSendFailed(format!(
            "WeChat message rejected (ret={}, errcode={})",
            value["ret"], value["errcode"]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_stores_credentials() {
        let client = Client::new();
        let api = WeixinApi::new(client, "https://ilinkai.weixin.qq.com/", "tok_abc");
        assert_eq!(api.base_url, "https://ilinkai.weixin.qq.com");
        assert_eq!(api.bot_token(), "tok_abc");
    }

    #[test]
    fn delivery_rejects_platform_errors_even_with_http_success() {
        assert!(check_send_result(&serde_json::json!({"ret":0})).is_ok());
        assert!(
            check_send_result(&serde_json::json!({"ret":-14}))
                .unwrap_err()
                .to_string()
                .contains("-14")
        );
        assert!(
            check_send_result(&serde_json::json!({"ret":0,"errcode":40001}))
                .unwrap_err()
                .to_string()
                .contains("40001")
        );
    }

    #[test]
    fn api_normalizes_trailing_slash() {
        let client = Client::new();
        let api = WeixinApi::new(client, "https://ilinkai.weixin.qq.com///", "tok");
        assert!(api.base_url.ends_with("com"));
    }

    #[test]
    fn api_generates_base64_decimal_uint32_header() {
        let client = Client::new();
        let api = WeixinApi::new(client, "https://example.com", "tok");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(api.wechat_uin())
            .unwrap();
        let decimal = std::str::from_utf8(&decoded).unwrap();
        assert!(decimal.bytes().all(|byte| byte.is_ascii_digit()));
        assert!(decimal.parse::<u32>().is_ok(), "UIN must be decimal uint32 text");
    }

    #[test]
    fn api_generates_different_uin_each_time() {
        let client1 = Client::new();
        let api1 = WeixinApi::new(client1, "https://example.com", "tok");
        let client2 = Client::new();
        let api2 = WeixinApi::new(client2, "https://example.com", "tok");
        // Extremely unlikely to collide (2^32 space)
        assert_ne!(api1.wechat_uin(), api2.wechat_uin());
    }
}
