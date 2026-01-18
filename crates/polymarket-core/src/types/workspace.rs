//! Workspace and onboarding types.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Workspace setup mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SetupMode {
    #[default]
    Manual,
    Automatic,
}

impl std::fmt::Display for SetupMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manual => write!(f, "manual"),
            Self::Automatic => write!(f, "automatic"),
        }
    }
}

impl std::str::FromStr for SetupMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "manual" => Ok(Self::Manual),
            "automatic" => Ok(Self::Automatic),
            _ => Err(format!("Invalid setup mode: {}", s)),
        }
    }
}

/// Role within a workspace (not platform role)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    #[default]
    Member,
    Viewer,
}

impl std::fmt::Display for WorkspaceRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Owner => write!(f, "owner"),
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
            Self::Viewer => write!(f, "viewer"),
        }
    }
}

impl std::str::FromStr for WorkspaceRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            "viewer" => Ok(Self::Viewer),
            _ => Err(format!("Invalid workspace role: {}", s)),
        }
    }
}

impl WorkspaceRole {
    /// Check if this role can modify the roster (add/remove/promote wallets)
    pub fn can_modify_roster(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Check if this role can manage members (invite/remove/change roles)
    pub fn can_manage_members(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Check if this role can access workspace settings
    pub fn can_access_settings(&self) -> bool {
        matches!(self, Self::Owner)
    }
}

/// Wallet tier in the roster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WalletTier {
    Active,
    #[default]
    Bench,
}

impl std::fmt::Display for WalletTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Bench => write!(f, "bench"),
        }
    }
}

impl std::str::FromStr for WalletTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "bench" => Ok(Self::Bench),
            _ => Err(format!("Invalid wallet tier: {}", s)),
        }
    }
}

/// Copy behavior setting for a wallet
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CopyBehavior {
    #[default]
    CopyAll,
    EventsOnly,
    ArbThreshold,
}

impl std::fmt::Display for CopyBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CopyAll => write!(f, "copy_all"),
            Self::EventsOnly => write!(f, "events_only"),
            Self::ArbThreshold => write!(f, "arb_threshold"),
        }
    }
}

impl std::str::FromStr for CopyBehavior {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "copy_all" => Ok(Self::CopyAll),
            "events_only" => Ok(Self::EventsOnly),
            "arb_threshold" => Ok(Self::ArbThreshold),
            _ => Err(format!("Invalid copy behavior: {}", s)),
        }
    }
}

/// Rotation action type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RotationAction {
    Promote,
    Demote,
    Replace,
    Add,
    Remove,
}

impl std::fmt::Display for RotationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Promote => write!(f, "promote"),
            Self::Demote => write!(f, "demote"),
            Self::Replace => write!(f, "replace"),
            Self::Add => write!(f, "add"),
            Self::Remove => write!(f, "remove"),
        }
    }
}

impl std::str::FromStr for RotationAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "promote" => Ok(Self::Promote),
            "demote" => Ok(Self::Demote),
            "replace" => Ok(Self::Replace),
            "add" => Ok(Self::Add),
            "remove" => Ok(Self::Remove),
            _ => Err(format!("Invalid rotation action: {}", s)),
        }
    }
}

/// A workspace (shared roster container)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,

    // Setup
    pub setup_mode: SetupMode,

    // Budget
    pub total_budget: Decimal,
    pub reserved_cash_pct: Decimal,

    // Auto-optimization
    pub auto_optimize_enabled: bool,
    pub optimization_interval_hours: i32,

    // Criteria thresholds
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,

    // Trading wallet
    pub trading_wallet_address: Option<String>,

    // Audit
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A member of a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub role: WorkspaceRole,
    pub joined_at: DateTime<Utc>,

    // Denormalized user info for display
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// User settings and onboarding state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettings {
    pub user_id: Uuid,
    pub onboarding_completed: bool,
    pub onboarding_step: i32,
    pub default_workspace_id: Option<Uuid>,
    pub preferences: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Wallet allocation within a workspace roster
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceWalletAllocation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub wallet_address: String,

    // Allocation
    pub allocation_pct: Decimal,
    pub max_position_size: Option<Decimal>,

    // Tier
    pub tier: WalletTier,

    // Auto-assignment
    pub auto_assigned: bool,
    pub auto_assigned_reason: Option<String>,

    // Backtest results
    pub backtest_roi: Option<Decimal>,
    pub backtest_sharpe: Option<Decimal>,
    pub backtest_win_rate: Option<Decimal>,

    // Copy settings
    pub copy_behavior: CopyBehavior,
    pub arb_threshold_pct: Option<Decimal>,

    // Audit
    pub added_by: Option<Uuid>,
    pub added_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A pending workspace invite
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInvite {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub email: String,
    pub role: WorkspaceRole,
    pub invited_by: Uuid,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,

    // Denormalized for display
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inviter_email: Option<String>,
}

/// An entry in the auto-rotation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRotationHistoryEntry {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub action: RotationAction,
    pub wallet_in: Option<String>,
    pub wallet_out: Option<String>,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub triggered_by: Option<Uuid>,
    pub notification_sent: bool,
    pub acknowledged: bool,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}
