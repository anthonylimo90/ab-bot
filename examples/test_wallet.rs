//! Test wallet connection for live trading.
//!
//! Run with:
//! ```
//! WALLET_PRIVATE_KEY=0x... cargo run --example test_wallet
//! ```

use auth::TradingWallet;
use polymarket_core::api::clob::AuthenticatedClobClient;
use polymarket_core::api::ClobClient;
use polymarket_core::signing::OrderSigner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("=== Wallet Connection Test ===\n");

    // Step 1: Load wallet from environment
    println!("1. Loading wallet from WALLET_PRIVATE_KEY...");
    let wallet = match TradingWallet::from_env() {
        Ok(w) => {
            println!("   ✓ Wallet loaded successfully");
            println!("   Address: {}", w.address_string());
            w
        }
        Err(e) => {
            println!("   ✗ Failed to load wallet: {}", e);
            println!("\n   Make sure WALLET_PRIVATE_KEY is set:");
            println!("   export WALLET_PRIVATE_KEY=0x...");
            return Err(e);
        }
    };

    // Step 2: Create order signer
    println!("\n2. Creating order signer...");
    let signer = OrderSigner::new(wallet.into_signer());
    println!("   ✓ Order signer created");
    println!("   Signer address: {:?}", signer.address());

    // Step 3: Test signing a message
    println!("\n3. Testing message signing...");
    let auth_signature = signer.sign_auth_message().await?;
    println!("   ✓ Auth message signed");
    println!(
        "   Signature: {}...{}",
        &auth_signature[..10],
        &auth_signature[auth_signature.len() - 8..]
    );

    // Step 4: Create authenticated client
    println!("\n4. Creating authenticated CLOB client...");
    let clob_client = ClobClient::new(None, None);
    let mut auth_client = AuthenticatedClobClient::new(clob_client, signer);
    println!("   ✓ Authenticated client created");
    println!("   Wallet address: {}", auth_client.address());

    // Step 5: Try to derive API credentials (optional - requires funded wallet)
    println!("\n5. Attempting to derive API credentials...");
    println!("   (This requires a funded wallet with Polymarket activity)");

    match auth_client.derive_api_key().await {
        Ok(creds) => {
            println!("   ✓ API credentials derived successfully!");
            println!(
                "   API Key: {}...",
                &creds.api_key[..12.min(creds.api_key.len())]
            );
            println!("\n   === LIVE TRADING READY ===");
        }
        Err(e) => {
            println!("   ⚠ Could not derive API credentials: {}", e);
            println!("\n   This is expected if:");
            println!("   - The wallet hasn't interacted with Polymarket before");
            println!("   - The wallet is not funded with MATIC/USDC");
            println!("   - There's a network issue");
            println!("\n   The wallet connection itself is working correctly.");
        }
    }

    // Step 6: Test reading market data (doesn't require auth)
    println!("\n6. Testing CLOB API connection (public endpoint)...");
    let public_client = ClobClient::new(None, None);
    match public_client.get_markets().await {
        Ok(markets) => {
            println!("   ✓ Connected to Polymarket CLOB API");
            println!("   Found {} active markets", markets.len());
            if let Some(market) = markets.first() {
                println!("   Sample market: {}", market.question);
            }
        }
        Err(e) => {
            println!("   ⚠ Could not fetch markets: {}", e);
        }
    }

    println!("\n=== Test Complete ===");
    Ok(())
}
