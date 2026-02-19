-- Extend dynamic_config_history action constraint for newly emitted runtime/manual actions.

ALTER TABLE dynamic_config_history
    DROP CONSTRAINT IF EXISTS dynamic_config_history_action_valid;

ALTER TABLE dynamic_config_history
    ADD CONSTRAINT dynamic_config_history_action_valid CHECK (
        action IN (
            'observed',
            'recommended',
            'applied',
            'rollback',
            'frozen',
            'skipped',
            'evaluation',
            'watchdog',
            'manual_update'
        )
    );
