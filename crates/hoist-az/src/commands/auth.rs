//! Authentication management commands

use anyhow::Result;

use hoist_client::auth::{AzCliAuth, EnvAuth};

use crate::cli::AuthCommands;

pub async fn run(cmd: AuthCommands) -> Result<()> {
    match cmd {
        AuthCommands::Login {
            service_principal,
            identity,
        } => login(service_principal, identity).await,
        AuthCommands::Status => status().await,
        AuthCommands::Logout => logout().await,
    }
}

async fn login(service_principal: bool, identity: bool) -> Result<()> {
    if service_principal {
        println!("Service principal authentication:");
        println!();
        println!("Set the following environment variables:");
        println!("  export AZURE_CLIENT_ID=<app-id>");
        println!("  export AZURE_CLIENT_SECRET=<secret>");
        println!("  export AZURE_TENANT_ID=<tenant-id>");
        println!();
        println!("Then run 'hoist auth status' to verify.");
        return Ok(());
    }

    if identity {
        println!("Managed identity authentication is used automatically when running in Azure.");
        println!("No login required.");
        return Ok(());
    }

    // Default: Azure CLI login
    println!("Opening browser for Azure CLI login...");
    println!();

    let status = std::process::Command::new("az").args(["login"]).status()?;

    if status.success() {
        println!();
        println!("Login successful! Run 'hoist auth status' to verify.");
    } else {
        anyhow::bail!("Azure CLI login failed");
    }

    Ok(())
}

async fn status() -> Result<()> {
    println!("Authentication Status");
    println!("=====================");
    println!();

    // Check environment variables
    if EnvAuth::is_configured() {
        println!("Environment Variables: Configured");
        println!("  AZURE_CLIENT_ID: set");
        println!("  AZURE_CLIENT_SECRET: set");
        println!("  AZURE_TENANT_ID: set");
    } else {
        println!("Environment Variables: Not configured");
    }

    println!();

    // Check Azure CLI
    match AzCliAuth::check_status() {
        Ok(status) => {
            println!("Azure CLI: Logged in");
            if let Some(user) = status.user {
                println!("  User: {}", user);
            }
            if let Some(sub) = status.subscription {
                println!("  Subscription: {}", sub);
            }
            if let Some(sub_id) = status.subscription_id {
                println!("  Subscription ID: {}", sub_id);
            }
        }
        Err(e) => {
            println!("Azure CLI: {}", e);
        }
    }

    println!();

    // Test token acquisition
    println!("Token Test:");
    match hoist_client::auth::get_auth_provider() {
        Ok(provider) => {
            println!("  Using: {}", provider.method_name());
            match provider.get_token() {
                Ok(_) => println!("  Status: Token acquired successfully"),
                Err(e) => println!("  Status: Failed - {}", e),
            }
        }
        Err(e) => {
            println!("  Status: No authentication available - {}", e);
        }
    }

    Ok(())
}

async fn logout() -> Result<()> {
    println!("Logging out of Azure CLI...");

    let status = std::process::Command::new("az").args(["logout"]).status()?;

    if status.success() {
        println!("Logged out successfully.");
        println!();
        println!("Note: Environment variables (AZURE_CLIENT_ID, etc.) are not cleared.");
        println!("Unset them manually if needed.");
    } else {
        anyhow::bail!("Azure CLI logout failed");
    }

    Ok(())
}
