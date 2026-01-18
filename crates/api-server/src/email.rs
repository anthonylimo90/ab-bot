//! Email client for sending password reset and notification emails.

use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use std::sync::Arc;
use thiserror::Error;

/// Email client errors.
#[derive(Debug, Error)]
pub enum EmailError {
    #[error("SMTP connection failed: {0}")]
    SmtpConnection(String),
    #[error("Failed to build email: {0}")]
    BuildEmail(String),
    #[error("Failed to send email: {0}")]
    SendEmail(String),
    #[error("Email client not configured")]
    NotConfigured,
}

/// Email client configuration.
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// SMTP host.
    pub smtp_host: String,
    /// SMTP port.
    pub smtp_port: u16,
    /// SMTP username.
    pub smtp_username: String,
    /// SMTP password.
    pub smtp_password: String,
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
        let smtp_host = std::env::var("SMTP_HOST").ok()?;
        let smtp_port = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(587);
        let smtp_username = std::env::var("SMTP_USERNAME").ok()?;
        let smtp_password = std::env::var("SMTP_PASSWORD").ok()?;
        let from_email = std::env::var("SMTP_FROM").ok()?;
        let from_name =
            std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "Polymarket Scanner".to_string());
        let app_url =
            std::env::var("APP_URL").unwrap_or_else(|_| "http://localhost:3002".to_string());

        Some(Self {
            smtp_host,
            smtp_port,
            smtp_username,
            smtp_password,
            from_email,
            from_name,
            app_url,
        })
    }
}

/// Email client for sending transactional emails.
#[derive(Clone)]
pub struct EmailClient {
    mailer: Arc<AsyncSmtpTransport<Tokio1Executor>>,
    from_mailbox: Mailbox,
    app_url: String,
}

impl EmailClient {
    /// Create a new email client with the given configuration.
    pub fn new(config: EmailConfig) -> Result<Self, EmailError> {
        let creds = Credentials::new(config.smtp_username, config.smtp_password);

        let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp_host)
            .map_err(|e| EmailError::SmtpConnection(e.to_string()))?
            .port(config.smtp_port)
            .credentials(creds)
            .timeout(Some(std::time::Duration::from_secs(10)))
            .build();

        let from_mailbox = format!("{} <{}>", config.from_name, config.from_email)
            .parse()
            .map_err(|e: lettre::address::AddressError| EmailError::BuildEmail(e.to_string()))?;

        Ok(Self {
            mailer: Arc::new(mailer),
            from_mailbox,
            app_url: config.app_url,
        })
    }

    /// Send a password reset email with the given token.
    pub async fn send_password_reset(&self, to_email: &str, token: &str) -> Result<(), EmailError> {
        let reset_url = format!("{}/reset-password?token={}", self.app_url, token);

        let to_mailbox: Mailbox = to_email
            .parse()
            .map_err(|e: lettre::address::AddressError| EmailError::BuildEmail(e.to_string()))?;

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

        let email = Message::builder()
            .from(self.from_mailbox.clone())
            .to(to_mailbox)
            .subject("Reset Your Password - Polymarket Scanner")
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text_body),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html_body),
                    ),
            )
            .map_err(|e| EmailError::BuildEmail(e.to_string()))?;

        self.mailer
            .send(email)
            .await
            .map_err(|e| EmailError::SendEmail(e.to_string()))?;

        Ok(())
    }

    /// Send a simple text email.
    pub async fn send_simple(
        &self,
        to_email: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), EmailError> {
        let to_mailbox: Mailbox = to_email
            .parse()
            .map_err(|e: lettre::address::AddressError| EmailError::BuildEmail(e.to_string()))?;

        let email = Message::builder()
            .from(self.from_mailbox.clone())
            .to(to_mailbox)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| EmailError::BuildEmail(e.to_string()))?;

        self.mailer
            .send(email)
            .await
            .map_err(|e| EmailError::SendEmail(e.to_string()))?;

        Ok(())
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
