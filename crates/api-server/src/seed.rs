//! Seed admin functionality for initial platform setup.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

/// Seed the initial platform admin user.
///
/// This function:
/// 1. Checks if any Admin users exist in the database
/// 2. If none exist, creates a new Admin user with the provided credentials
/// 3. If an admin already exists, returns an error
pub async fn seed_admin(pool: &PgPool, email: &str, password: &str) -> anyhow::Result<()> {
    // Validate inputs
    if !email.contains('@') || email.len() < 5 {
        anyhow::bail!("Invalid email address format");
    }

    if password.len() < 8 {
        anyhow::bail!("Password must be at least 8 characters long");
    }

    // Check if any admin users already exist
    let existing_admin: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM users WHERE role = 2 LIMIT 1")
            .fetch_optional(pool)
            .await?;

    if existing_admin.is_some() {
        anyhow::bail!(
            "An admin user already exists. Use the dashboard to manage users or reset the database."
        );
    }

    // Check if email is already in use
    let existing_email: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await?;

    if existing_email.is_some() {
        anyhow::bail!("A user with this email already exists");
    }

    // Hash the password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("Password hashing failed: {}", e))?
        .to_string();

    // Create the admin user
    let user_id = Uuid::new_v4();
    let role: i16 = 2; // Admin role
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, role, name, created_at, updated_at)
        VALUES ($1, $2, $3, $4, 'Platform Admin', $5, $5)
        "#,
    )
    .bind(user_id)
    .bind(email)
    .bind(&password_hash)
    .bind(role)
    .bind(now)
    .execute(pool)
    .await?;

    tracing::info!(
        user_id = %user_id,
        email = %email,
        "Platform admin user created successfully"
    );

    println!("Successfully created platform admin:");
    println!("  Email: {}", email);
    println!("  User ID: {}", user_id);
    println!();
    println!("You can now log in to the dashboard with these credentials.");

    Ok(())
}

/// Seed admin from environment variables (for Docker/CI).
///
/// Reads SEED_ADMIN_EMAIL and SEED_ADMIN_PASSWORD from environment.
pub async fn seed_admin_from_env(pool: &PgPool) -> anyhow::Result<()> {
    let email = std::env::var("SEED_ADMIN_EMAIL").ok();
    let password = std::env::var("SEED_ADMIN_PASSWORD").ok();

    match (email, password) {
        (Some(email), Some(password)) => {
            tracing::info!("Seeding admin from environment variables...");
            seed_admin(pool, &email, &password).await
        }
        _ => {
            tracing::debug!("SEED_ADMIN_EMAIL/PASSWORD not set, skipping auto-seed");
            Ok(())
        }
    }
}
