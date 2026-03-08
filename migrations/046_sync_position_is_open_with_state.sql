-- Keep legacy is_open flags aligned with canonical lifecycle state.
-- Closed (4) and entry_failed (5) are not active; everything else is treated as active.

UPDATE positions
SET is_open = CASE
    WHEN state IN (4, 5) THEN FALSE
    ELSE TRUE
END
WHERE is_open IS DISTINCT FROM CASE
    WHEN state IN (4, 5) THEN FALSE
    ELSE TRUE
END;
