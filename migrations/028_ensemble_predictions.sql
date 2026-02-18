-- Store ensemble prediction probabilities at wallet selection time
-- for later calibration against actual copy trade outcomes.
CREATE TABLE IF NOT EXISTS ensemble_predictions (
    address TEXT NOT NULL,
    workspace_id UUID NOT NULL,
    predicted_prob DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (address, workspace_id)
);

CREATE INDEX IF NOT EXISTS idx_ensemble_predictions_created ON ensemble_predictions (created_at);
