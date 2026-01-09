//! Polymarket Core Library
//!
//! Shared types, API clients, and database models for the Polymarket Scanner system.

pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod types;

pub use error::{Error, Result};
