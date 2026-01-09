//! Authentication and Security
//!
//! JWT authentication, API key management, key vault, RBAC, and audit logging.

pub mod api_key;
pub mod audit;
pub mod jwt;
pub mod key_vault;
pub mod rbac;

pub use api_key::ApiKeyAuth;
pub use audit::{AuditAction, AuditEvent, AuditLogger};
pub use jwt::{Claims, JwtAuth, UserRole};
pub use key_vault::{KeyVault, KeyVaultProvider, WalletKey};
pub use rbac::{
    Action, DefaultRoles, Permission, PermissionConditions, RbacManager,
    Resource, Role, TimeWindow,
};
