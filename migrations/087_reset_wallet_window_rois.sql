-- Reset wallet window ROIs that were never computed per window.
--
-- ProfitabilityAnalyzer::store_metrics used to bind the same 30-day value to
-- roi_30d, roi_90d and roi_all_time (and the value itself was derived from raw
-- cash flows rather than realized P&L). It now writes each window's column only
-- from the window that was actually computed, and only the 30-day window is
-- computed by the metrics calculator.
--
-- The existing roi_90d / roi_all_time values are therefore mislabelled copies
-- of an old 30-day figure and nothing would ever overwrite them. Reset them to
-- the column default (0 = not computed). roi_30d and the other 30-day columns
-- are refreshed by the metrics calculator on its next cycle and are untouched.
UPDATE wallet_success_metrics
SET roi_90d = 0,
    roi_all_time = 0
WHERE roi_90d <> 0
   OR roi_all_time <> 0;
