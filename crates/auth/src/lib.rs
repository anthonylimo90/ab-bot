//! Authentication and Security
//!
//! JWT authentication, the wallet key vault, and audit logging.

pub mod audit;
pub mod audit_storage_pg;
pub mod jwt;
pub mod key_vault;
pub mod wallet;

pub use audit::{AuditAction, AuditEvent, AuditFilter, AuditLogger, AuditStorage};
pub use audit_storage_pg::PostgresAuditStorage;
pub use jwt::{Claims, JwtAuth, UserRole};
pub use key_vault::{KeyVault, KeyVaultProvider, WalletKey};
pub use wallet::TradingWallet;
