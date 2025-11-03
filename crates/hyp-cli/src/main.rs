use anyhow::Result;
use clap::Parser;
use hyp_cli::cli::{Cli, Commands};
use hyp_cli::command;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize rustls crypto provider
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Failed to install default crypto provider"))?;

    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version {} => {
            command::version();
        }
        Commands::CreateIsm {
            ism_type,
            validators,
            threshold,
        } => {
            command::create_ism(ism_type, validators, threshold).await?;
        }
        Commands::CreateHook {
            hook_type,
            mailbox_id,
        } => {
            command::create_hook(hook_type, mailbox_id).await?;
        }
        Commands::CreateMailbox {
            ism_id,
            local_domain,
            default_hook,
            required_hook,
        } => {
            command::create_mailbox(ism_id, local_domain, default_hook, required_hook).await?;
        }
        Commands::CreateWarpToken {
            mailbox_id,
            ism_id,
            denom,
        } => {
            command::create_warp_token(mailbox_id, ism_id, denom).await?;
        }
        Commands::EnrollRouter {
            token_id,
            remote_domain,
            remote_contract,
        } => {
            command::enroll_router(token_id, remote_domain, remote_contract).await?;
        }
        Commands::AnnounceValidator {
            validator,
            storage_location,
            signature,
            mailbox_id,
        } => {
            command::announce_validator(validator, storage_location, signature, mailbox_id).await?;
        }
        Commands::DeployStack {
            ism_id,
            local_domain,
            use_merkle_hook,
            denom,
        } => {
            command::deploy_stack(ism_id, local_domain, use_merkle_hook, denom).await?;
        }
    }

    Ok(())
}
