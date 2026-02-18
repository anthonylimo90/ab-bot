//! Integration tests for component interactions.
//!
//! These tests verify that the major components work together correctly.

use rust_decimal::Decimal;

/// Test that the trading engine config and order executor work together.
#[test]
fn test_executor_config_integration() {
    use trading_engine::executor::ExecutorConfig;

    let config = ExecutorConfig {
        max_order_size: Decimal::new(1000, 0),
        fee_rate: Decimal::new(2, 2), // 2%
        live_trading: false,
        timeout_ms: 5000,
        max_retries: 2,
        retry_base_delay_ms: 50,
        retry_max_delay_ms: 1000,
        ..Default::default()
    };

    assert_eq!(config.max_order_size, Decimal::new(1000, 0));
    assert_eq!(config.timeout_ms, 5000);
    assert_eq!(config.max_retries, 2);
    assert!(!config.live_trading);
}

/// Test circuit breaker configuration and state.
#[test]
fn test_circuit_breaker_config() {
    use risk_manager::{CircuitBreaker, CircuitBreakerConfig};

    let config = CircuitBreakerConfig {
        max_daily_loss: Decimal::new(500, 0),
        max_drawdown_pct: Decimal::new(15, 2), // 15%
        max_consecutive_losses: 3,
        cooldown_minutes: 30,
        enabled: true,
        ..Default::default()
    };

    let breaker = CircuitBreaker::new(config.clone());

    assert!(!breaker.is_tripped());
}

/// Test stop-loss rule creation and trigger detection.
#[test]
fn test_stop_loss_rule_creation() {
    use risk_manager::{StopLossRule, StopType};
    use uuid::Uuid;

    // Test fixed stop
    let mut fixed_rule = StopLossRule::new(
        Uuid::new_v4(),
        "market1".to_string(),
        "yes_token".to_string(),
        Decimal::new(60, 2), // Entry at 0.60
        Decimal::new(100, 0),
        StopType::fixed(Decimal::new(50, 2)), // Trigger at 0.50
    );

    fixed_rule.activate();
    assert!(fixed_rule.activated);
    assert!(!fixed_rule.executed);

    // Should not trigger at 0.55
    assert!(!fixed_rule.is_triggered(Decimal::new(55, 2)));

    // Should trigger at 0.50
    assert!(fixed_rule.is_triggered(Decimal::new(50, 2)));

    // Should trigger below 0.50
    assert!(fixed_rule.is_triggered(Decimal::new(45, 2)));
}

/// Test trailing stop functionality.
#[test]
fn test_trailing_stop_updates() {
    use risk_manager::{StopLossRule, StopType};
    use uuid::Uuid;

    let mut trailing_rule = StopLossRule::new(
        Uuid::new_v4(),
        "market1".to_string(),
        "yes_token".to_string(),
        Decimal::new(50, 2), // Entry at 0.50
        Decimal::new(100, 0),
        StopType::trailing(Decimal::new(10, 2)), // 10% trailing
    );

    trailing_rule.activate();

    // Update peak to 0.60
    trailing_rule.update_peak(Decimal::new(60, 2));

    // Trigger should now be at 0.54 (0.60 * 0.90)
    let trigger = trailing_rule.current_trigger_price();
    assert_eq!(trigger, Some(Decimal::new(54, 2)));

    // Should not trigger at 0.56
    assert!(!trailing_rule.is_triggered(Decimal::new(56, 2)));

    // Should trigger at 0.54
    assert!(trailing_rule.is_triggered(Decimal::new(54, 2)));
}

/// Test position state transitions.
#[test]
fn test_position_state_transitions() {
    use polymarket_core::types::{ExitStrategy, Position, PositionState};

    let mut position = Position::new(
        "test_market".to_string(),
        Decimal::new(40, 2), // YES at 0.40
        Decimal::new(55, 2), // NO at 0.55
        Decimal::new(100, 0),
        ExitStrategy::ExitOnCorrection,
    );

    // Initial state should be Pending
    assert_eq!(position.state, PositionState::Pending);

    // Transition to Open
    position.mark_open().unwrap();
    assert_eq!(position.state, PositionState::Open);

    // Transition to ExitReady
    position.mark_exit_ready().unwrap();
    assert_eq!(position.state, PositionState::ExitReady);

    // Transition to Closing
    position.mark_closing().unwrap();
    assert_eq!(position.state, PositionState::Closing);
}

/// Test position failure states.
#[test]
fn test_position_failure_states() {
    use polymarket_core::types::{ExitStrategy, FailureReason, Position, PositionState};

    let mut position = Position::new(
        "test_market".to_string(),
        Decimal::new(40, 2),
        Decimal::new(55, 2),
        Decimal::new(100, 0),
        ExitStrategy::HoldToResolution,
    );

    // Mark entry failed
    position.mark_entry_failed(FailureReason::OrderTimeout { elapsed_ms: 5000 });
    assert_eq!(position.state, PositionState::EntryFailed);
    assert!(position.failure_reason.is_some());

    // Create another position for exit failure test
    let mut position2 = Position::new(
        "test_market".to_string(),
        Decimal::new(40, 2),
        Decimal::new(55, 2),
        Decimal::new(100, 0),
        ExitStrategy::ExitOnCorrection,
    );

    position2.mark_open().unwrap();
    position2.mark_exit_ready().unwrap();

    // Mark exit failed
    position2.mark_exit_failed(FailureReason::ConnectivityError {
        message: "timeout".to_string(),
    });
    assert_eq!(position2.state, PositionState::ExitFailed);
    assert_eq!(position2.retry_count, 1);
    assert!(position2.can_retry());
}

/// Test position P&L calculations.
#[test]
fn test_position_pnl_calculation() {
    use polymarket_core::types::{ExitStrategy, Position};

    let mut position = Position::new(
        "test_market".to_string(),
        Decimal::new(40, 2), // YES at 0.40
        Decimal::new(55, 2), // NO at 0.55
        Decimal::new(100, 0),
        ExitStrategy::ExitOnCorrection,
    );

    // Entry cost: (0.40 + 0.55) * 100 = 95
    assert_eq!(position.entry_cost(), Decimal::new(95, 0));

    // Update P&L with exit prices
    let yes_bid = Decimal::new(48, 2); // 0.48
    let no_bid = Decimal::new(52, 2); // 0.52
    let fee = Decimal::new(2, 2); // 2%

    position.update_pnl(yes_bid, no_bid, fee);

    // Exit value: (0.48 + 0.52) * 100 = 100
    // Entry cost: 95
    // Entry fees: 0.02 * 95 = 1.90
    // Exit fees: 0.02 * 100 = 2.00
    // Expected P&L: 100 - 95 - 1.90 - 2.00 = 1.10
    assert_eq!(position.unrealized_pnl, Decimal::new(110, 2)); // 1.10
}

/// Test RBAC permission checking.
#[tokio::test]
async fn test_rbac_permissions() {
    use auth::rbac::{Action, Permission, RbacManager, Resource, Role};

    let rbac = RbacManager::new();

    // Create a trader role using the correct API
    let mut trader_role = Role::new("custom_trader", "Custom trader with specific permissions");
    trader_role.add_permission(Permission::new(Resource::Order, Action::Create));
    trader_role.add_permission(Permission::new(Resource::Order, Action::Read));
    trader_role.add_permission(Permission::new(Resource::Position, Action::Read));

    rbac.add_role(trader_role).await.unwrap();
    rbac.assign_role("test_user", "custom_trader")
        .await
        .unwrap();

    // Trader should be able to create orders
    let can_create = rbac
        .has_permission("test_user", &Resource::Order, &Action::Create)
        .await;
    assert!(can_create);

    // Trader should be able to read positions
    let can_read = rbac
        .has_permission("test_user", &Resource::Position, &Action::Read)
        .await;
    assert!(can_read);

    // Trader should NOT be able to delete orders (not granted)
    let cannot_delete = rbac
        .has_permission("test_user", &Resource::Order, &Action::Delete)
        .await;
    assert!(!cannot_delete);
}

/// Test key vault encryption round-trip.
#[tokio::test]
async fn test_key_vault_encryption() {
    use auth::{KeyVault, KeyVaultProvider};

    let vault = KeyVault::new(
        KeyVaultProvider::Memory,
        b"test-master-key-32bytes!".to_vec(),
    );

    let private_key = b"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let address = "0xABCD1234";

    // Store the key
    vault.store_wallet_key(address, private_key).await.unwrap();

    // Retrieve and verify
    let retrieved = vault.get_wallet_key(address).await.unwrap();
    assert_eq!(retrieved, Some(private_key.to_vec()));

    // Verify case-insensitive lookup
    let retrieved_lower = vault.get_wallet_key("0xabcd1234").await.unwrap();
    assert_eq!(retrieved_lower, Some(private_key.to_vec()));

    // Remove the key
    vault.remove_wallet_key(address).await.unwrap();
    let after_remove = vault.get_wallet_key(address).await.unwrap();
    assert!(after_remove.is_none());
}

/// Test JWT authentication flow.
#[test]
fn test_jwt_auth_flow() {
    use auth::jwt::{JwtAuth, JwtConfig, UserRole};

    let config = JwtConfig {
        secret: "test-secret-key-for-jwt-signing".to_string(),
        issuer: Some("test".to_string()),
        audience: Some("test".to_string()),
        expiry_hours: 24,
    };
    let auth = JwtAuth::new(config);

    // Create a token for a trader
    let token = auth.create_token("user123", UserRole::Trader).unwrap();
    assert!(!token.is_empty());

    // Validate the token
    let claims = auth.validate_token(&token).unwrap();
    assert_eq!(claims.sub, "user123");
    assert_eq!(claims.role, UserRole::Trader);

    // Check permissions - trader should have trader role
    let has_trader = auth.check_permission(&token, UserRole::Trader).unwrap();
    assert!(has_trader);

    // Check permissions - trader should have viewer role (lower)
    let has_viewer = auth.check_permission(&token, UserRole::Viewer).unwrap();
    assert!(has_viewer);
}

/// Test circuit breaker state persistence setup.
#[tokio::test]
async fn test_circuit_breaker_state_tracking() {
    use risk_manager::{CircuitBreaker, CircuitBreakerConfig, TripReason};

    let config = CircuitBreakerConfig {
        max_daily_loss: Decimal::new(100, 0),
        max_consecutive_losses: 3,
        enabled: true,
        ..Default::default()
    };

    let breaker = CircuitBreaker::new(config);

    // Record some losses
    breaker
        .record_trade(Decimal::new(-20, 0), false)
        .await
        .unwrap();
    breaker
        .record_trade(Decimal::new(-30, 0), false)
        .await
        .unwrap();

    // Check state
    let state = breaker.state().await;
    assert_eq!(state.daily_pnl, Decimal::new(-50, 0));
    assert_eq!(state.consecutive_losses, 2);
    assert!(!state.tripped);

    // Third loss should trigger consecutive losses trip
    let reason = breaker
        .record_trade(Decimal::new(-10, 0), false)
        .await
        .unwrap();

    assert_eq!(reason, Some(TripReason::ConsecutiveLosses));
    assert!(breaker.is_tripped());
}

/// Test advanced compound stop conditions.
#[test]
fn test_compound_stop_conditions() {
    use risk_manager::{CompoundLogic, CompoundStop, StopCondition, StopContext};
    use uuid::Uuid;

    // Create context with the correct fields
    let context = StopContext {
        current_price: Decimal::new(45, 2),            // 0.45
        entry_price: Decimal::new(50, 2),              // 0.50
        unrealized_pnl: Decimal::new(-5, 0),           // $5 loss
        current_volatility: Some(Decimal::new(15, 2)), // 15%
        current_volume: Some(Decimal::new(10000, 0)),
        position_age_hours: 2,
    };

    // Test OR logic: either condition triggers
    let mut or_stop = CompoundStop::new(
        Uuid::new_v4(),
        vec![
            StopCondition::PriceBelow {
                price: Decimal::new(40, 2), // Not triggered (current is 0.45)
            },
            StopCondition::LossExceeds {
                amount: Decimal::new(3, 0), // Triggered (we have $5 loss)
            },
        ],
        CompoundLogic::Or,
    );
    or_stop.activate();
    assert!(or_stop.check(&context));

    // Test AND logic: both conditions must trigger
    let mut and_stop = CompoundStop::new(
        Uuid::new_v4(),
        vec![
            StopCondition::PriceBelow {
                price: Decimal::new(46, 2), // Triggered (current is 0.45)
            },
            StopCondition::LossExceeds {
                amount: Decimal::new(10, 0), // Not triggered (only $5 loss)
            },
        ],
        CompoundLogic::And,
    );
    and_stop.activate();
    assert!(!and_stop.check(&context));

    // Test AND logic with both conditions met
    let mut and_stop_triggered = CompoundStop::new(
        Uuid::new_v4(),
        vec![
            StopCondition::PriceBelow {
                price: Decimal::new(46, 2), // Triggered
            },
            StopCondition::LossExceeds {
                amount: Decimal::new(4, 0), // Triggered (we have $5 loss)
            },
        ],
        CompoundLogic::And,
    );
    and_stop_triggered.activate();
    assert!(and_stop_triggered.check(&context));
}

/// Test position age calculation.
#[test]
fn test_position_age() {
    use polymarket_core::types::{ExitStrategy, Position};

    let position = Position::new(
        "test_market".to_string(),
        Decimal::new(50, 2),
        Decimal::new(50, 2),
        Decimal::new(100, 0),
        ExitStrategy::HoldToResolution,
    );

    // Position should be very young
    let age = position.age_secs();
    assert!(age < 2); // Less than 2 seconds old
}

/// Test volatility stop ATR calculation.
#[test]
fn test_volatility_stop() {
    use chrono::Utc;
    use risk_manager::advanced_stops::{PriceBar, VolatilityStop};

    let mut vol_stop = VolatilityStop::new(3, Decimal::new(2, 0)); // 3-period, 2x multiplier

    // Add enough price bars to calculate ATR
    vol_stop.add_bar(PriceBar {
        high: Decimal::new(52, 2),
        low: Decimal::new(48, 2),
        close: Decimal::new(50, 2),
        timestamp: Utc::now(),
    });
    vol_stop.add_bar(PriceBar {
        high: Decimal::new(53, 2),
        low: Decimal::new(49, 2),
        close: Decimal::new(51, 2),
        timestamp: Utc::now(),
    });
    vol_stop.add_bar(PriceBar {
        high: Decimal::new(54, 2),
        low: Decimal::new(50, 2),
        close: Decimal::new(52, 2),
        timestamp: Utc::now(),
    });
    vol_stop.add_bar(PriceBar {
        high: Decimal::new(55, 2),
        low: Decimal::new(51, 2),
        close: Decimal::new(53, 2),
        timestamp: Utc::now(),
    });

    // ATR should be calculated now
    assert!(vol_stop.current_atr().is_some());

    // Get stop level for entry at 0.50
    let stop_level = vol_stop.get_stop_level(Decimal::new(50, 2));
    assert!(stop_level.is_some());
    // Stop should be below entry price
    assert!(stop_level.unwrap() < Decimal::new(50, 2));
}

/// Test step trailing stop.
#[test]
fn test_step_trailing_stop() {
    use risk_manager::StepTrailingStop;
    use uuid::Uuid;

    let mut stop = StepTrailingStop::new(
        Uuid::new_v4(),
        Decimal::new(50, 2), // Entry at 0.50
        Decimal::new(5, 2),  // 0.05 steps
        Decimal::new(3, 2),  // 3% offset
    );
    stop.activate();

    // Price moves up to 0.55 (one step)
    assert!(!stop.update(Decimal::new(55, 2)));
    assert_eq!(stop.highest_step, 1);

    // Price moves up to 0.60 (two steps)
    assert!(!stop.update(Decimal::new(60, 2)));
    assert_eq!(stop.highest_step, 2);

    // Price falls - should eventually trigger
    // The stop price is at entry + (steps - 1) * step_size
    assert!(stop.update(Decimal::new(48, 2))); // Below initial stop
}

/// Test break-even stop.
#[test]
fn test_break_even_stop() {
    use risk_manager::BreakEvenStop;
    use uuid::Uuid;

    let mut stop = BreakEvenStop::new(
        Uuid::new_v4(),
        Decimal::new(50, 2), // Entry at 0.50
        Decimal::new(5, 2),  // 5% profit trigger
        Decimal::new(1, 3),  // 0.1% buffer
    );
    stop.activate();

    // Price at entry - no break-even yet
    assert!(!stop.update(Decimal::new(50, 2)));
    assert!(!stop.triggered_to_break_even);

    // Price up 5% to 0.525 - should trigger break-even
    assert!(!stop.update(Decimal::new(525, 3)));
    assert!(stop.triggered_to_break_even);
    assert!(stop.stop_price.is_some());

    // Price falls to stop level (just below break-even)
    assert!(stop.update(Decimal::new(499, 3))); // Below 0.50 + buffer
}

/// Test default roles from RBAC.
#[tokio::test]
async fn test_default_roles() {
    use auth::rbac::{Action, DefaultRoles, RbacManager, Resource};

    let rbac = RbacManager::new();

    // Assign built-in platform_admin role
    rbac.assign_role("admin_user", "platform_admin")
        .await
        .unwrap();

    // Admin should have all permissions
    assert!(
        rbac.has_permission("admin_user", &Resource::Position, &Action::Create)
            .await
    );
    assert!(
        rbac.has_permission("admin_user", &Resource::SystemConfig, &Action::Configure)
            .await
    );
    assert!(
        rbac.has_permission("admin_user", &Resource::User, &Action::Manage)
            .await
    );

    // Test viewer role
    rbac.assign_role("viewer_user", "viewer").await.unwrap();
    assert!(
        rbac.has_permission("viewer_user", &Resource::Position, &Action::Read)
            .await
    );
    assert!(
        !rbac
            .has_permission("viewer_user", &Resource::Position, &Action::Create)
            .await
    );

    // Verify default roles exist
    let viewer = DefaultRoles::viewer();
    assert!(viewer.system_role);

    let trader = DefaultRoles::trader();
    assert!(trader.inherits.contains(&"viewer".to_string()));
}
