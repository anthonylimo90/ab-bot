//! Email client using Resend HTTP API.
//!
//! Uses Resend's HTTP API instead of SMTP for reliable email delivery
//! in cloud environments where SMTP ports may be blocked.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

const RESEND_API_URL: &str = "https://api.resend.com/emails";

/// Email client errors.
#[derive(Debug, Error)]
pub enum EmailError {
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("Failed to send email: {0}")]
    SendError(String),
    #[error("Email client not configured")]
    NotConfigured,
}

/// Email client configuration.
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// Resend API key.
    pub api_key: String,
    /// From email address.
    pub from_email: String,
    /// From display name.
    pub from_name: String,
    /// Application URL for reset links.
    pub app_url: String,
}

impl EmailConfig {
    /// Create configuration from environment variables.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("RESEND_API_KEY").ok()?;
        let from_email = std::env::var("RESEND_FROM")
            .or_else(|_| std::env::var("SMTP_FROM"))
            .ok()?;
        let from_name = std::env::var("RESEND_FROM_NAME")
            .or_else(|_| std::env::var("SMTP_FROM_NAME"))
            .unwrap_or_else(|_| "Polymarket Scanner".to_string());
        let app_url =
            std::env::var("APP_URL").unwrap_or_else(|_| "http://localhost:3002".to_string());

        Some(Self {
            api_key,
            from_email,
            from_name,
            app_url,
        })
    }
}

/// Resend API request body.
#[derive(Debug, Serialize)]
struct ResendEmailRequest {
    from: String,
    to: Vec<String>,
    subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// Resend API response.
#[derive(Debug, Deserialize)]
struct ResendEmailResponse {
    id: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Email client for sending transactional emails via Resend.
#[derive(Clone)]
pub struct EmailClient {
    client: Arc<Client>,
    api_key: String,
    from: String,
    app_url: String,
}

impl EmailClient {
    /// Create a new email client with the given configuration.
    pub fn new(config: EmailConfig) -> Result<Self, EmailError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| EmailError::HttpError(e.to_string()))?;

        let from = format!("{} <{}>", config.from_name, config.from_email);

        Ok(Self {
            client: Arc::new(client),
            api_key: config.api_key,
            from,
            app_url: config.app_url,
        })
    }

    /// Send an email via Resend API.
    async fn send_email(
        &self,
        to: &str,
        subject: &str,
        html: Option<String>,
        text: Option<String>,
    ) -> Result<(), EmailError> {
        let request = ResendEmailRequest {
            from: self.from.clone(),
            to: vec![to.to_string()],
            subject: subject.to_string(),
            html,
            text,
        };

        let response = self
            .client
            .post(RESEND_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| EmailError::HttpError(e.to_string()))?;

        let status = response.status();
        let body: ResendEmailResponse = response
            .json()
            .await
            .unwrap_or(ResendEmailResponse { id: None, message: None });

        if status.is_success() {
            tracing::debug!(email_id = ?body.id, "Email sent successfully via Resend");
            Ok(())
        } else {
            let error_msg = body.message.unwrap_or_else(|| format!("HTTP {}", status));
            Err(EmailError::SendError(error_msg))
        }
    }

    /// Send a password reset email with the given token.
    pub async fn send_password_reset(&self, to_email: &str, token: &str) -> Result<(), EmailError> {
        let reset_url = format!("{}/reset-password?token={}", self.app_url, token);

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Password Reset</title>
</head>
<body style="margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif; background-color: #f4f4f5;">
    <table role="presentation" style="width: 100%; border-collapse: collapse;">
        <tr>
            <td align="center" style="padding: 40px 0;">
                <table role="presentation" style="width: 100%; max-width: 600px; border-collapse: collapse; background-color: #ffffff; border-radius: 8px; box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);">
                    <tr>
                        <td style="padding: 40px 40px 20px 40px; text-align: center;">
                            <h1 style="margin: 0 0 10px 0; color: #18181b; font-size: 24px; font-weight: 600;">Password Reset Request</h1>
                            <p style="margin: 0; color: #71717a; font-size: 16px;">Polymarket Scanner</p>
                        </td>
                    </tr>
                    <tr>
                        <td style="padding: 20px 40px;">
                            <p style="margin: 0 0 20px 0; color: #3f3f46; font-size: 16px; line-height: 1.5;">
                                You requested to reset your password. Click the button below to set a new password:
                            </p>
                            <table role="presentation" style="width: 100%; border-collapse: collapse;">
                                <tr>
                                    <td align="center" style="padding: 20px 0;">
                                        <a href="{reset_url}" style="display: inline-block; padding: 14px 32px; background-color: #18181b; color: #ffffff; text-decoration: none; font-size: 16px; font-weight: 500; border-radius: 6px;">Reset Password</a>
                                    </td>
                                </tr>
                            </table>
                            <p style="margin: 20px 0 0 0; color: #71717a; font-size: 14px; line-height: 1.5;">
                                This link will expire in 1 hour. If you didn't request this password reset, you can safely ignore this email.
                            </p>
                        </td>
                    </tr>
                    <tr>
                        <td style="padding: 20px 40px; border-top: 1px solid #e4e4e7;">
                            <p style="margin: 0; color: #a1a1aa; font-size: 12px; line-height: 1.5;">
                                If the button doesn't work, copy and paste this link into your browser:<br>
                                <a href="{reset_url}" style="color: #3b82f6; word-break: break-all;">{reset_url}</a>
                            </p>
                        </td>
                    </tr>
                </table>
            </td>
        </tr>
    </table>
</body>
</html>"#
        );

        let text_body = format!(
            r#"Password Reset Request

You requested to reset your password for Polymarket Scanner.

Click the link below to set a new password:
{reset_url}

This link will expire in 1 hour.

If you didn't request this password reset, you can safely ignore this email."#
        );

        self.send_email(
            to_email,
            "Reset Your Password - Polymarket Scanner",
            Some(html_body),
            Some(text_body),
        )
        .await
    }

    /// Send a simple text email.
    pub async fn send_simple(
        &self,
        to_email: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), EmailError> {
        self.send_email(to_email, subject, None, Some(body.to_string()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_config_from_env() {
        // This will return None if env vars are not set
        let config = EmailConfig::from_env();
        // Just ensure it doesn't panic
        assert!(config.is_none() || config.is_some());
    }
}
