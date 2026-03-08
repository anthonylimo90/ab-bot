use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedStrategyMode {
    Disabled,
    Paper,
    Live,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StrategyModeStatus {
    pub strategy: String,
    pub mode: ResolvedStrategyMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StrategyModeInputs {
    pub arb_enabled: bool,
    pub workspace_arb_enabled: bool,
    pub workspace_live_enabled: bool,
    pub live_executor: bool,
    pub live_ready: bool,
    pub quant_executor_enabled: bool,
    pub flow_signal_enabled: bool,
    pub mean_reversion_signal_enabled: bool,
    pub cross_market_signal_enabled: bool,
    pub resolution_signal_enabled: bool,
}

pub fn resolve_strategy_modes(inputs: &StrategyModeInputs) -> Vec<StrategyModeStatus> {
    vec![
        resolve_arb_mode(inputs),
        resolve_quant_mode("flow", inputs.flow_signal_enabled, inputs),
        resolve_quant_mode(
            "mean_reversion",
            inputs.mean_reversion_signal_enabled,
            inputs,
        ),
        resolve_quant_mode("cross_market", inputs.cross_market_signal_enabled, inputs),
        resolve_quant_mode(
            "resolution_proximity",
            inputs.resolution_signal_enabled,
            inputs,
        ),
    ]
}

fn resolve_arb_mode(inputs: &StrategyModeInputs) -> StrategyModeStatus {
    if !inputs.arb_enabled {
        return StrategyModeStatus {
            strategy: "arb".to_string(),
            mode: ResolvedStrategyMode::Disabled,
            reason: Some("Arb executor disabled".to_string()),
        };
    }

    if !inputs.workspace_arb_enabled {
        return StrategyModeStatus {
            strategy: "arb".to_string(),
            mode: ResolvedStrategyMode::Disabled,
            reason: Some("Workspace arb_auto_execute=false".to_string()),
        };
    }

    if !inputs.workspace_live_enabled {
        return StrategyModeStatus {
            strategy: "arb".to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some("Workspace live_trading_enabled=false".to_string()),
        };
    }

    if !inputs.live_executor {
        return StrategyModeStatus {
            strategy: "arb".to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some("Global order executor is in paper mode".to_string()),
        };
    }

    if !inputs.live_ready {
        return StrategyModeStatus {
            strategy: "arb".to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some(
                "Live executor is enabled but wallet/API credentials are not ready".to_string(),
            ),
        };
    }

    StrategyModeStatus {
        strategy: "arb".to_string(),
        mode: ResolvedStrategyMode::Live,
        reason: None,
    }
}

fn resolve_quant_mode(
    strategy: &str,
    signal_enabled: bool,
    inputs: &StrategyModeInputs,
) -> StrategyModeStatus {
    if !signal_enabled {
        return StrategyModeStatus {
            strategy: strategy.to_string(),
            mode: ResolvedStrategyMode::Disabled,
            reason: Some("Signal generator disabled".to_string()),
        };
    }

    if !inputs.quant_executor_enabled {
        return StrategyModeStatus {
            strategy: strategy.to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some(
                "Quant executor disabled; signals still generated for paper evaluation".to_string(),
            ),
        };
    }

    if !inputs.workspace_live_enabled {
        return StrategyModeStatus {
            strategy: strategy.to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some("Workspace live_trading_enabled=false".to_string()),
        };
    }

    if !inputs.live_executor {
        return StrategyModeStatus {
            strategy: strategy.to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some("Global order executor is in paper mode".to_string()),
        };
    }

    if !inputs.live_ready {
        return StrategyModeStatus {
            strategy: strategy.to_string(),
            mode: ResolvedStrategyMode::Paper,
            reason: Some(
                "Live executor is enabled but wallet/API credentials are not ready".to_string(),
            ),
        };
    }

    StrategyModeStatus {
        strategy: strategy.to_string(),
        mode: ResolvedStrategyMode::Live,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_inputs() -> StrategyModeInputs {
        StrategyModeInputs {
            arb_enabled: true,
            workspace_arb_enabled: true,
            workspace_live_enabled: true,
            live_executor: true,
            live_ready: true,
            quant_executor_enabled: true,
            flow_signal_enabled: true,
            mean_reversion_signal_enabled: true,
            cross_market_signal_enabled: false,
            resolution_signal_enabled: true,
        }
    }

    #[test]
    fn resolves_disabled_signal_generators() {
        let modes = resolve_strategy_modes(&base_inputs());
        let cross_market = modes
            .into_iter()
            .find(|item| item.strategy == "cross_market")
            .unwrap();

        assert_eq!(cross_market.mode, ResolvedStrategyMode::Disabled);
    }

    #[test]
    fn resolves_quant_paper_when_executor_disabled() {
        let mut inputs = base_inputs();
        inputs.quant_executor_enabled = false;
        let modes = resolve_strategy_modes(&inputs);
        let flow = modes
            .into_iter()
            .find(|item| item.strategy == "flow")
            .unwrap();

        assert_eq!(flow.mode, ResolvedStrategyMode::Paper);
    }

    #[test]
    fn resolves_arb_live_when_ready() {
        let modes = resolve_strategy_modes(&base_inputs());
        let arb = modes
            .into_iter()
            .find(|item| item.strategy == "arb")
            .unwrap();

        assert_eq!(arb.mode, ResolvedStrategyMode::Live);
    }

    #[test]
    fn resolves_quant_paper_when_workspace_live_disabled() {
        let mut inputs = base_inputs();
        inputs.workspace_live_enabled = false;
        let modes = resolve_strategy_modes(&inputs);
        let flow = modes
            .into_iter()
            .find(|item| item.strategy == "flow")
            .unwrap();

        assert_eq!(flow.mode, ResolvedStrategyMode::Paper);
        assert_eq!(
            flow.reason.as_deref(),
            Some("Workspace live_trading_enabled=false")
        );
    }

    #[test]
    fn resolves_arb_disabled_when_workspace_arb_disabled() {
        let mut inputs = base_inputs();
        inputs.workspace_arb_enabled = false;
        let modes = resolve_strategy_modes(&inputs);
        let arb = modes
            .into_iter()
            .find(|item| item.strategy == "arb")
            .unwrap();

        assert_eq!(arb.mode, ResolvedStrategyMode::Disabled);
        assert_eq!(
            arb.reason.as_deref(),
            Some("Workspace arb_auto_execute=false")
        );
    }
}
