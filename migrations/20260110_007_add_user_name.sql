-- Add name column to users table for display purposes
-- Migration: 007 - Add user name column

ALTER TABLE users ADD COLUMN IF NOT EXISTS name VARCHAR(255);
