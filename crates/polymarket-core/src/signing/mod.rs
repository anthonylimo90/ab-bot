//! Signing module for Polymarket CLOB orders.
//!
//! This module provides EIP-712 typed data signing for orders and
//! authentication messages required by the Polymarket CLOB API.
//!
//! # Architecture
//!
//! ```text
//! TradingWallet (auth crate)
//!       │
//!       ▼
//! OrderSigner ─── signs ──► SignedOrder
//!       │                        │
//!       │                        ▼
//!       │               AuthenticatedClobClient
//!       │                        │
//!       └── L1 auth ────────────►│
//!                                ▼
//!                        Polymarket CLOB API
//! ```
//!
//! # Example
//!
//! ```ignore
//! use polymarket_core::signing::{OrderSigner, OrderSide};
//! use alloy_signer_local::PrivateKeySigner;
//! use rust_decimal::Decimal;
//!
//! // Create signer from private key
//! let private_key = PrivateKeySigner::from_str("0x...")?;
//! let signer = OrderSigner::new(private_key);
//!
//! // Build and sign an order
//! let order = signer
//!     .order_builder()
//!     .token_id(U256::from(12345))
//!     .side(OrderSide::Buy)
//!     .price(Decimal::new(50, 2))  // 0.50
//!     .size(Decimal::from(100))    // 100 USDC
//!     .expires_in(3600)            // 1 hour
//!     .build()
//!     .unwrap();
//!
//! let signed_order = signer.sign_order(&order).await?;
//! ```

pub mod domain;
pub mod order_types;
pub mod signer;

pub use domain::{
    Eip712Domain, OrderSide, SignatureType,
    CTF_EXCHANGE_ADDRESS, NEG_RISK_ADAPTER_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS,
    POLYGON_AMOY_CHAIN_ID, POLYGON_CHAIN_ID, USDC_ADDRESS,
};

pub use order_types::{OrderBuilder, OrderData, SignedOrder};

pub use signer::OrderSigner;
