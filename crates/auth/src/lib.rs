//! Authentication and Security
//!
//! JWT authentication, API key management, key vault, RBAC, and audit logging.

pub mod api_key;
pub mod audit;
pub mod audit_storage_pg;
pub mod jwt;
pub mod key_vault;
pub mod rbac;
pub mod wallet;

pub use api_key::ApiKeyAuth;
pub use audit::{AuditAction, AuditEvent, AuditFilter, AuditLogger, AuditStorage};
pub use audit_storage_pg::PostgresAuditStorage;
pub use jwt::{Claims, JwtAuth, UserRole};
pub use key_vault::{KeyVault, KeyVaultProvider, WalletKey};
pub use rbac::{
    Action, DefaultRoles, Permission, PermissionConditions, RbacManager, Resource, Role, TimeWindow,
};
pub use wallet::TradingWallet;
