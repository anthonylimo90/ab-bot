//! Role-Based Access Control (RBAC) system.
//!
//! Comprehensive permission management with roles, resources, and actions.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Permission action types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // Basic CRUD
    Create,
    Read,
    Update,
    Delete,

    // Trading specific
    Execute,
    Cancel,
    Close,

    // Admin specific
    Manage,
    Configure,
    Export,

    // Special
    All,
}

impl Action {
    /// Check if this action includes another.
    pub fn includes(&self, other: &Action) -> bool {
        *self == Action::All || *self == *other
    }
}

/// Resource types in the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    // Core resources
    Position,
    Order,
    Market,
    Wallet,

    // Trading resources
    StopLoss,
    CopyTrading,
    Backtest,

    // Configuration
    Strategy,
    RiskSettings,
    SystemConfig,

    // User management
    User,
    Role,
    ApiKey,

    // Data
    AuditLog,
    Analytics,
    Reports,

    // Wildcard for specific resource ID
    Specific { resource_type: String, resource_id: String },

    // All resources
    All,
}

impl Resource {
    /// Create a specific resource reference.
    pub fn specific(resource_type: &str, resource_id: &str) -> Self {
        Self::Specific {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
        }
    }

    /// Check if this resource matches another.
    pub fn matches(&self, other: &Resource) -> bool {
        match (self, other) {
            (Resource::All, _) => true,
            (_, Resource::All) => true,
            (a, b) if a == b => true,
            (Resource::Specific { resource_type: t1, .. }, Resource::Specific { resource_type: t2, .. }) => {
                t1 == t2
            }
            _ => false,
        }
    }
}

/// A permission grants an action on a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission {
    pub resource: Resource,
    pub action: Action,
    /// Optional conditions for the permission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<PermissionConditions>,
}

impl Permission {
    /// Create a new permission.
    pub fn new(resource: Resource, action: Action) -> Self {
        Self {
            resource,
            action,
            conditions: None,
        }
    }

    /// Add conditions to the permission.
    pub fn with_conditions(mut self, conditions: PermissionConditions) -> Self {
        self.conditions = Some(conditions);
        self
    }

    /// Check if this permission grants access to the requested action on resource.
    pub fn grants(&self, resource: &Resource, action: &Action) -> bool {
        self.resource.matches(resource) && self.action.includes(action)
    }
}

/// Conditions that can be attached to permissions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionConditions {
    /// Time-based restrictions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_window: Option<TimeWindow>,
    /// IP restrictions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_whitelist: Option<Vec<String>>,
    /// Maximum amount for trading operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<f64>,
    /// Require MFA for this permission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_mfa: Option<bool>,
}

/// Time window for permission validity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeWindow {
    /// Start hour (0-23).
    pub start_hour: u8,
    /// End hour (0-23).
    pub end_hour: u8,
    /// Allowed days (0 = Sunday, 6 = Saturday).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_days: Option<Vec<u8>>,
}

impl TimeWindow {
    /// Check if current time is within the window.
    pub fn is_active(&self) -> bool {
        let now = Utc::now();
        let hour = now.time().hour() as u8;
        let day = now.weekday().num_days_from_sunday() as u8;

        let in_hours = if self.start_hour <= self.end_hour {
            hour >= self.start_hour && hour < self.end_hour
        } else {
            // Wraps around midnight
            hour >= self.start_hour || hour < self.end_hour
        };

        let in_days = self.allowed_days
            .as_ref()
            .map(|days| days.contains(&day))
            .unwrap_or(true);

        in_hours && in_days
    }
}

/// A role is a named collection of permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub permissions: HashSet<Permission>,
    /// Parent roles (inherits their permissions).
    pub inherits: Vec<String>,
    /// Is this a system role that cannot be modified?
    pub system_role: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Role {
    /// Create a new role.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            description: description.into(),
            permissions: HashSet::new(),
            inherits: Vec::new(),
            system_role: false,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a permission to the role.
    pub fn add_permission(&mut self, permission: Permission) {
        self.permissions.insert(permission);
        self.updated_at = Utc::now();
    }

    /// Remove a permission from the role.
    pub fn remove_permission(&mut self, permission: &Permission) -> bool {
        let removed = self.permissions.remove(permission);
        if removed {
            self.updated_at = Utc::now();
        }
        removed
    }

    /// Add a parent role to inherit from.
    pub fn inherit_from(&mut self, role_name: impl Into<String>) {
        self.inherits.push(role_name.into());
        self.updated_at = Utc::now();
    }

    /// Mark as system role.
    pub fn as_system_role(mut self) -> Self {
        self.system_role = true;
        self
    }
}

/// Default roles for the system.
pub struct DefaultRoles;

impl DefaultRoles {
    /// Create the Viewer role.
    pub fn viewer() -> Role {
        let mut role = Role::new("viewer", "Read-only access to dashboards and data")
            .as_system_role();

        // Read access to most resources
        role.add_permission(Permission::new(Resource::Position, Action::Read));
        role.add_permission(Permission::new(Resource::Order, Action::Read));
        role.add_permission(Permission::new(Resource::Market, Action::Read));
        role.add_permission(Permission::new(Resource::Wallet, Action::Read));
        role.add_permission(Permission::new(Resource::Analytics, Action::Read));
        role.add_permission(Permission::new(Resource::Backtest, Action::Read));

        role
    }

    /// Create the Trader role.
    pub fn trader() -> Role {
        let mut role = Role::new("trader", "Can execute trades and manage positions")
            .as_system_role();

        role.inherit_from("viewer");

        // Trading permissions
        role.add_permission(Permission::new(Resource::Position, Action::Create));
        role.add_permission(Permission::new(Resource::Position, Action::Update));
        role.add_permission(Permission::new(Resource::Position, Action::Close));
        role.add_permission(Permission::new(Resource::Order, Action::Create));
        role.add_permission(Permission::new(Resource::Order, Action::Execute));
        role.add_permission(Permission::new(Resource::Order, Action::Cancel));
        role.add_permission(Permission::new(Resource::StopLoss, Action::Create));
        role.add_permission(Permission::new(Resource::StopLoss, Action::Update));
        role.add_permission(Permission::new(Resource::StopLoss, Action::Delete));
        role.add_permission(Permission::new(Resource::CopyTrading, Action::Read));
        role.add_permission(Permission::new(Resource::CopyTrading, Action::Update));
        role.add_permission(Permission::new(Resource::Backtest, Action::Create));
        role.add_permission(Permission::new(Resource::Backtest, Action::Execute));

        role
    }

    /// Create the Admin role.
    pub fn admin() -> Role {
        let mut role = Role::new("admin", "Full access including configuration")
            .as_system_role();

        role.inherit_from("trader");

        // Admin permissions
        role.add_permission(Permission::new(Resource::All, Action::All));
        role.add_permission(Permission::new(Resource::User, Action::Manage));
        role.add_permission(Permission::new(Resource::Role, Action::Manage));
        role.add_permission(Permission::new(Resource::ApiKey, Action::Manage));
        role.add_permission(Permission::new(Resource::SystemConfig, Action::Configure));
        role.add_permission(Permission::new(Resource::RiskSettings, Action::Configure));
        role.add_permission(Permission::new(Resource::AuditLog, Action::Read));
        role.add_permission(Permission::new(Resource::Reports, Action::Export));

        role
    }

    /// Create the Copy Trader role (specialized trader).
    pub fn copy_trader() -> Role {
        let mut role = Role::new("copy_trader", "Can manage copy trading operations");

        role.inherit_from("viewer");

        role.add_permission(Permission::new(Resource::CopyTrading, Action::Create));
        role.add_permission(Permission::new(Resource::CopyTrading, Action::Update));
        role.add_permission(Permission::new(Resource::CopyTrading, Action::Delete));
        role.add_permission(Permission::new(Resource::Wallet, Action::Create));
        role.add_permission(Permission::new(Resource::Wallet, Action::Update));

        role
    }

    /// Create the Risk Manager role.
    pub fn risk_manager() -> Role {
        let mut role = Role::new("risk_manager", "Can configure risk settings");

        role.inherit_from("viewer");

        role.add_permission(Permission::new(Resource::RiskSettings, Action::Read));
        role.add_permission(Permission::new(Resource::RiskSettings, Action::Update));
        role.add_permission(Permission::new(Resource::StopLoss, Action::Manage));
        role.add_permission(Permission::new(Resource::Position, Action::Close)); // Emergency close

        role
    }

    /// Get all default roles.
    pub fn all() -> Vec<Role> {
        vec![
            Self::viewer(),
            Self::trader(),
            Self::admin(),
            Self::copy_trader(),
            Self::risk_manager(),
        ]
    }
}

/// RBAC manager for permission checking.
pub struct RbacManager {
    roles: Arc<RwLock<HashMap<String, Role>>>,
    user_roles: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl RbacManager {
    /// Create a new RBAC manager with default roles.
    pub fn new() -> Self {
        let mut roles = HashMap::new();
        for role in DefaultRoles::all() {
            roles.insert(role.name.clone(), role);
        }

        Self {
            roles: Arc::new(RwLock::new(roles)),
            user_roles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a custom role.
    pub async fn add_role(&self, role: Role) -> Result<()> {
        let mut roles = self.roles.write().await;
        if roles.contains_key(&role.name) {
            return Err(anyhow!("Role {} already exists", role.name));
        }
        roles.insert(role.name.clone(), role);
        Ok(())
    }

    /// Update an existing role.
    pub async fn update_role(&self, role: Role) -> Result<()> {
        let mut roles = self.roles.write().await;
        if let Some(existing) = roles.get(&role.name) {
            if existing.system_role {
                return Err(anyhow!("Cannot modify system role {}", role.name));
            }
        }
        roles.insert(role.name.clone(), role);
        Ok(())
    }

    /// Delete a role.
    pub async fn delete_role(&self, name: &str) -> Result<()> {
        let mut roles = self.roles.write().await;
        if let Some(role) = roles.get(name) {
            if role.system_role {
                return Err(anyhow!("Cannot delete system role {}", name));
            }
        }
        roles.remove(name);
        Ok(())
    }

    /// Get a role by name.
    pub async fn get_role(&self, name: &str) -> Option<Role> {
        let roles = self.roles.read().await;
        roles.get(name).cloned()
    }

    /// List all roles.
    pub async fn list_roles(&self) -> Vec<Role> {
        let roles = self.roles.read().await;
        roles.values().cloned().collect()
    }

    /// Assign a role to a user.
    pub async fn assign_role(&self, user_id: &str, role_name: &str) -> Result<()> {
        // Verify role exists
        let roles = self.roles.read().await;
        if !roles.contains_key(role_name) {
            return Err(anyhow!("Role {} does not exist", role_name));
        }
        drop(roles);

        let mut user_roles = self.user_roles.write().await;
        user_roles
            .entry(user_id.to_string())
            .or_default()
            .push(role_name.to_string());
        Ok(())
    }

    /// Remove a role from a user.
    pub async fn revoke_role(&self, user_id: &str, role_name: &str) -> Result<()> {
        let mut user_roles = self.user_roles.write().await;
        if let Some(roles) = user_roles.get_mut(user_id) {
            roles.retain(|r| r != role_name);
        }
        Ok(())
    }

    /// Get all roles for a user.
    pub async fn get_user_roles(&self, user_id: &str) -> Vec<String> {
        let user_roles = self.user_roles.read().await;
        user_roles.get(user_id).cloned().unwrap_or_default()
    }

    /// Check if a user has permission for an action on a resource.
    pub async fn has_permission(
        &self,
        user_id: &str,
        resource: &Resource,
        action: &Action,
    ) -> bool {
        let user_role_names = self.get_user_roles(user_id).await;
        let roles = self.roles.read().await;

        // Collect all permissions including inherited
        let permissions = self.collect_permissions(&user_role_names, &roles);

        // Check if any permission grants access
        for perm in permissions {
            if perm.grants(resource, action) {
                // Check conditions if present
                if let Some(conditions) = &perm.conditions {
                    if let Some(time_window) = &conditions.time_window {
                        if !time_window.is_active() {
                            continue;
                        }
                    }
                }
                return true;
            }
        }

        false
    }

    /// Collect all permissions for a set of roles, including inherited.
    fn collect_permissions(
        &self,
        role_names: &[String],
        roles: &HashMap<String, Role>,
    ) -> Vec<Permission> {
        let mut permissions = Vec::new();
        let mut visited = HashSet::new();

        fn collect_recursive(
            role_name: &str,
            roles: &HashMap<String, Role>,
            permissions: &mut Vec<Permission>,
            visited: &mut HashSet<String>,
        ) {
            if visited.contains(role_name) {
                return; // Avoid cycles
            }
            visited.insert(role_name.to_string());

            if let Some(role) = roles.get(role_name) {
                // Add direct permissions
                permissions.extend(role.permissions.iter().cloned());

                // Recursively add inherited permissions
                for parent in &role.inherits {
                    collect_recursive(parent, roles, permissions, visited);
                }
            }
        }

        for role_name in role_names {
            collect_recursive(role_name, roles, &mut permissions, &mut visited);
        }

        permissions
    }

    /// Get all effective permissions for a user.
    pub async fn get_effective_permissions(&self, user_id: &str) -> Vec<Permission> {
        let user_role_names = self.get_user_roles(user_id).await;
        let roles = self.roles.read().await;
        self.collect_permissions(&user_role_names, &roles)
    }
}

impl Default for RbacManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Permission check result with details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCheckResult {
    pub allowed: bool,
    pub user_id: String,
    pub resource: Resource,
    pub action: Action,
    pub matching_permission: Option<Permission>,
    pub denial_reason: Option<String>,
    pub checked_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_roles() {
        let viewer = DefaultRoles::viewer();
        assert!(viewer.system_role);
        assert!(viewer.permissions.iter().all(|p| p.action == Action::Read));

        let trader = DefaultRoles::trader();
        assert!(trader.inherits.contains(&"viewer".to_string()));

        let admin = DefaultRoles::admin();
        assert!(admin.permissions.iter().any(|p| p.action == Action::All));
    }

    #[tokio::test]
    async fn test_permission_grants() {
        let perm = Permission::new(Resource::Position, Action::Read);
        assert!(perm.grants(&Resource::Position, &Action::Read));
        assert!(!perm.grants(&Resource::Position, &Action::Create));
        assert!(!perm.grants(&Resource::Order, &Action::Read));

        let all_perm = Permission::new(Resource::All, Action::All);
        assert!(all_perm.grants(&Resource::Position, &Action::Create));
        assert!(all_perm.grants(&Resource::Order, &Action::Delete));
    }

    #[tokio::test]
    async fn test_rbac_manager() {
        let manager = RbacManager::new();

        // Assign viewer role
        manager.assign_role("user1", "viewer").await.unwrap();

        // Check permissions
        assert!(manager.has_permission("user1", &Resource::Position, &Action::Read).await);
        assert!(!manager.has_permission("user1", &Resource::Position, &Action::Create).await);

        // Assign trader role
        manager.assign_role("user1", "trader").await.unwrap();
        assert!(manager.has_permission("user1", &Resource::Position, &Action::Create).await);
    }

    #[tokio::test]
    async fn test_role_inheritance() {
        let manager = RbacManager::new();
        manager.assign_role("admin_user", "admin").await.unwrap();

        // Admin should have viewer and trader permissions through inheritance
        assert!(manager.has_permission("admin_user", &Resource::Position, &Action::Read).await);
        assert!(manager.has_permission("admin_user", &Resource::Order, &Action::Execute).await);
        assert!(manager.has_permission("admin_user", &Resource::SystemConfig, &Action::Configure).await);
    }

    #[tokio::test]
    async fn test_custom_role() {
        let manager = RbacManager::new();

        let mut custom = Role::new("analyst", "Data analyst role");
        custom.add_permission(Permission::new(Resource::Analytics, Action::Read));
        custom.add_permission(Permission::new(Resource::Reports, Action::Read));
        custom.add_permission(Permission::new(Resource::Reports, Action::Export));

        manager.add_role(custom).await.unwrap();
        manager.assign_role("analyst1", "analyst").await.unwrap();

        assert!(manager.has_permission("analyst1", &Resource::Analytics, &Action::Read).await);
        assert!(manager.has_permission("analyst1", &Resource::Reports, &Action::Export).await);
        assert!(!manager.has_permission("analyst1", &Resource::Position, &Action::Read).await);
    }

    #[tokio::test]
    async fn test_cannot_modify_system_role() {
        let manager = RbacManager::new();

        let modified_viewer = Role::new("viewer", "Modified viewer");
        let result = manager.update_role(modified_viewer).await;
        assert!(result.is_err());

        let result = manager.delete_role("viewer").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_time_window() {
        let window = TimeWindow {
            start_hour: 9,
            end_hour: 17,
            allowed_days: Some(vec![1, 2, 3, 4, 5]), // Mon-Fri
        };

        // This test depends on current time, so we just verify it runs
        let _ = window.is_active();
    }

    #[test]
    fn test_resource_matching() {
        assert!(Resource::All.matches(&Resource::Position));
        assert!(Resource::Position.matches(&Resource::All));
        assert!(Resource::Position.matches(&Resource::Position));
        assert!(!Resource::Position.matches(&Resource::Order));

        let specific1 = Resource::specific("position", "123");
        let specific2 = Resource::specific("position", "456");
        assert!(specific1.matches(&specific2)); // Same type
    }
}
