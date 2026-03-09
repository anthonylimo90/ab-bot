# Learning Model Artifact Format

The trained-model runtime supports local JSON artifacts for:

- `trained_linear_probability_v1`
- `trained_linear_regression_v1`

Artifacts can be supplied either through:

- `learning_model_registry.artifact_uri` pointing at an absolute JSON file path
- `learning_model_registry.metrics.artifact` containing the same JSON object inline

## JSON schema

```json
{
  "intercept": -0.42,
  "weights": {
    "confidence": 2.3,
    "freshness": 0.9,
    "expected_edge_bps": 0.004
  },
  "threshold": 0.62,
  "positive_action": "execute",
  "negative_action": "skip",
  "transform": "sigmoid",
  "clip_min": 0.0,
  "clip_max": 1.0
}
```

## Supported fields

- `intercept`: optional float, defaults to `0.0`
- `weights`: map of feature name to coefficient
- `threshold`: optional decision threshold; otherwise target defaults are used
- `positive_action`: optional action when the score clears the threshold
- `negative_action`: optional action when the score fails the threshold
- `transform`: one of `sigmoid`, `identity`, or `clamp_0_1`
- `clip_min` / `clip_max`: optional post-transform clamps

## Available arb features

- `signal_age_secs`
- `yes_ask`
- `no_ask`
- `total_cost`
- `gross_profit`
- `net_profit`
- `gross_profit_ratio`
- `net_profit_ratio`
- `live_ready`

## Available quant features

- `confidence`
- `suggested_size_usd`
- `age_secs`
- `time_to_expiry_secs`
- `freshness`
- `expected_edge_bps`
- `normalized_edge`
- `min_confidence`
- `max_signal_age_secs`
- `direction_buy_yes`
- `direction_buy_no`
- `kind_flow`
- `kind_cross_market`
- `kind_mean_reversion`
- `kind_resolution_proximity`
