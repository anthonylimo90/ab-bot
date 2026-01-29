//! JWT authentication for API access.

use anyhow::Result;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Platform-level user roles for authorization.
/// Note: This is distinct from WorkspaceRole which controls per-workspace access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UserRole {
    /// Read-only access to dashboards and data.
    #[default]
    Viewer,
    /// Can execute trades and manage positions.
    Trader,
    /// Platform administrator - manages workspaces and users platform-wide.
    /// Cannot access trading dashboard (uses separate admin portal).
    PlatformAdmin,
}

impl UserRole {
    /// Check if this role can execute trades.
    pub fn can_trade(&self) -> bool {
        matches!(self, UserRole::Trader | UserRole::PlatformAdmin)
    }

    /// Check if this role can modify configuration.
    pub fn can_configure(&self) -> bool {
        matches!(self, UserRole::PlatformAdmin)
    }

    /// Check if this role can view data.
    pub fn can_view(&self) -> bool {
        true // All roles can view
    }
}

/// JWT claims payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID).
    pub sub: String,
    /// User's email.
    pub email: Option<String>,
    /// User's wallet address (if authenticated via wallet).
    pub wallet_address: Option<String>,
    /// User's role.
    pub role: UserRole,
    /// Issued at timestamp.
    pub iat: i64,
    /// Expiration timestamp.
    pub exp: i64,
    /// JWT ID (unique identifier for this token).
    pub jti: String,
}

impl Claims {
    /// Create new claims for a user.
    pub fn new(user_id: impl Into<String>, role: UserRole, expiry_hours: i64) -> Self {
        let now = Utc::now();
        Self {
            sub: user_id.into(),
            email: None,
            wallet_address: None,
            role,
            iat: now.timestamp(),
            exp: (now + Duration::hours(expiry_hours)).timestamp(),
            jti: Uuid::new_v4().to_string(),
        }
    }

    /// Add email to claims.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Add wallet address to claims.
    pub fn with_wallet_address(mut self, address: impl Into<String>) -> Self {
        self.wallet_address = Some(address.into());
        self
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() > self.exp
    }

    /// Get remaining validity duration.
    pub fn remaining_validity(&self) -> Option<Duration> {
        let remaining = self.exp - Utc::now().timestamp();
        if remaining > 0 {
            Some(Duration::seconds(remaining))
        } else {
            None
        }
    }
}

/// Configuration for JWT authentication.
#[derive(Clone)]
pub struct JwtConfig {
    /// Secret key for signing tokens.
    pub secret: String,
    /// Token expiry duration in hours.
    pub expiry_hours: i64,
    /// Issuer claim.
    pub issuer: Option<String>,
    /// Audience claim.
    pub audience: Option<String>,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret: "change-me-in-production".to_string(),
            expiry_hours: 24,
            issuer: None,
            audience: None,
        }
    }
}

/// JWT authentication handler.
pub struct JwtAuth {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    config: JwtConfig,
    validation: Validation,
}

impl JwtAuth {
    /// Create a new JWT authenticator.
    pub fn new(config: JwtConfig) -> Self {
        let encoding_key = EncodingKey::from_secret(config.secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(config.secret.as_bytes());

        let mut validation = Validation::default();
        if let Some(ref iss) = config.issuer {
            validation.set_issuer(&[iss]);
        }
        if let Some(ref aud) = config.audience {
            validation.set_audience(&[aud]);
        }

        Self {
            encoding_key,
            decoding_key,
            config,
            validation,
        }
    }

    /// Create a new token for a user.
    pub fn create_token(&self, user_id: &str, role: UserRole) -> Result<String> {
        let claims = Claims::new(user_id, role, self.config.expiry_hours);
        let token = encode(&Header::default(), &claims, &self.encoding_key)?;
        Ok(token)
    }

    /// Create a token with custom claims.
    pub fn create_token_with_claims(&self, claims: &Claims) -> Result<String> {
        let token = encode(&Header::default(), claims, &self.encoding_key)?;
        Ok(token)
    }

    /// Validate and decode a token.
    pub fn validate_token(&self, token: &str) -> Result<Claims> {
        let token_data = decode::<Claims>(token, &self.decoding_key, &self.validation)?;
        Ok(token_data.claims)
    }

    /// Refresh a token (create new token with extended expiry).
    pub fn refresh_token(&self, token: &str) -> Result<String> {
        let claims = self.validate_token(token)?;

        // Create new claims with same user but new expiry
        let new_claims = Claims::new(&claims.sub, claims.role, self.config.expiry_hours)
            .with_email(claims.email.unwrap_or_default());

        self.create_token_with_claims(&new_claims)
    }

    /// Check if a token grants a specific permission.
    pub fn check_permission(&self, token: &str, required_role: UserRole) -> Result<bool> {
        let claims = self.validate_token(token)?;

        let has_permission = match required_role {
            UserRole::Viewer => true, // All roles have viewer permissions
            UserRole::Trader => claims.role.can_trade(),
            UserRole::PlatformAdmin => claims.role.can_configure(),
        };

        Ok(has_permission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_auth() -> JwtAuth {
        JwtAuth::new(JwtConfig {
            secret: "test-secret-key-12345".to_string(),
            expiry_hours: 1,
            ..Default::default()
        })
    }

    #[test]
    fn test_create_and_validate_token() {
        let auth = create_test_auth();

        let token = auth.create_token("user123", UserRole::Trader).unwrap();
        let claims = auth.validate_token(&token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.role, UserRole::Trader);
        assert!(!claims.is_expired());
    }

    #[test]
    fn test_role_permissions() {
        assert!(UserRole::PlatformAdmin.can_trade());
        assert!(UserRole::PlatformAdmin.can_configure());
        assert!(UserRole::PlatformAdmin.can_view());

        assert!(UserRole::Trader.can_trade());
        assert!(!UserRole::Trader.can_configure());
        assert!(UserRole::Trader.can_view());

        assert!(!UserRole::Viewer.can_trade());
        assert!(!UserRole::Viewer.can_configure());
        assert!(UserRole::Viewer.can_view());
    }

    #[test]
    fn test_check_permission() {
        let auth = create_test_auth();

        let admin_token = auth.create_token("admin", UserRole::PlatformAdmin).unwrap();
        let trader_token = auth.create_token("trader", UserRole::Trader).unwrap();
        let viewer_token = auth.create_token("viewer", UserRole::Viewer).unwrap();

        // Admin can do everything
        assert!(auth
            .check_permission(&admin_token, UserRole::Viewer)
            .unwrap());
        assert!(auth
            .check_permission(&admin_token, UserRole::Trader)
            .unwrap());
        assert!(auth
            .check_permission(&admin_token, UserRole::PlatformAdmin)
            .unwrap());

        // Trader can trade and view
        assert!(auth
            .check_permission(&trader_token, UserRole::Viewer)
            .unwrap());
        assert!(auth
            .check_permission(&trader_token, UserRole::Trader)
            .unwrap());
        assert!(!auth
            .check_permission(&trader_token, UserRole::PlatformAdmin)
            .unwrap());

        // Viewer can only view
        assert!(auth
            .check_permission(&viewer_token, UserRole::Viewer)
            .unwrap());
        assert!(!auth
            .check_permission(&viewer_token, UserRole::Trader)
            .unwrap());
        assert!(!auth
            .check_permission(&viewer_token, UserRole::PlatformAdmin)
            .unwrap());
    }

    #[test]
    fn test_invalid_token() {
        let auth = create_test_auth();

        let result = auth.validate_token("invalid-token");
        assert!(result.is_err());
    }

    #[test]
    fn test_refresh_token() {
        let auth = create_test_auth();

        let token = auth.create_token("user123", UserRole::Trader).unwrap();
        let new_token = auth.refresh_token(&token).unwrap();

        let claims = auth.validate_token(&new_token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.role, UserRole::Trader);
    }
}
