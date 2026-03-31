-- Fix exit_strategy column type: production DB has INT4 (integer) but schema
-- defines SMALLINT (INT2). This causes sqlx decoding errors since Rust reads
-- the column as i16.
-- Values are only 0 (HoldToResolution) and 1 (ExitOnCorrection), so the cast
-- is lossless.

ALTER TABLE positions
    ALTER COLUMN exit_strategy TYPE SMALLINT USING exit_strategy::SMALLINT;
