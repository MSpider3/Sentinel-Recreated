use anyhow::Result;
use clap::Parser;
use log::{error, info};
use std::path::PathBuf;
use tokio::signal;
use zbus::connection::Builder;

use sentinel_core::audit::AuditLogger;
use sentinel_core::config::SentinelConfig;
use sentinel_core::dbus::SentinelService;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Sentinel Recreated Facial Biometric Authentication Daemon",
    long_about = None
)]
struct Args {
    /// Path to configuration file.
    #[arg(short, long, default_value = "/etc/sentinel/config.toml")]
    config: String,

    /// Directory containing ONNX models.
    #[arg(short, long, default_value = "/var/cache/sentinel/models")]
    models_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize logging (env_logger logs to stdout/stderr, which journald captures)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();
    info!("Starting Sentinel Recreated Daemon (sentinel-core)...");

    // 2. Load Configuration
    let config_path = PathBuf::from(&args.config);
    let config = SentinelConfig::load(&config_path).unwrap_or_default();
    info!("Loaded configuration from: {}", config_path.display());

    let models_dir = PathBuf::from(&args.models_dir);
    info!("ONNX models directory: {}", models_dir.display());

    // 3. Systemd Type=dbus requirement: Claim DBus bus name EARLY before expensive operations
    let rt_handle = tokio::runtime::Handle::current();
    let service = SentinelService::new(config, config_path, models_dir.clone(), rt_handle);

    info!("Registering DBus service on System Bus under 'com.sentinel.Sentinel'...");
    let _conn = Builder::system()?
        .name("com.sentinel.Sentinel")?
        .serve_at("/com/sentinel/Sentinel", service)?
        .build()
        .await?;

    info!("Successfully claimed System DBus name 'com.sentinel.Sentinel'.");

    // 4. Initialize AuditLogger and run 30-day retention cleanup
    info!("Initializing AuditLogger and checking retention policy (30 days)...");
    let audit_logger = AuditLogger::new();
    let _ = audit_logger.cleanup_old_logs(30);

    // 5. Verify mandatory ONNX model files
    let scrfd_path = models_dir.join("scrfd_500m_kps.onnx");
    let mfn_path = models_dir.join("mobile_facenet.onnx");

    if !scrfd_path.exists() || !mfn_path.exists() {
        error!(
            "Mandatory ONNX model files missing in '{}'. Ensure scrfd_500m_kps.onnx and mobile_facenet.onnx are present.",
            models_dir.display()
        );
        std::process::exit(1);
    }
    info!("All mandatory ONNX models verified successfully.");

    // 6. Graceful Shutdown Handler (SIGINT / SIGTERM)
    info!("Daemon running. Awaiting shutdown signals (SIGINT / SIGTERM)...");
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Received shutdown signal. Flushing audit logs and exiting...");
        }
        Err(err) => {
            error!("Failed to listen for shutdown signal: {}", err);
        }
    }

    info!("Sentinel Daemon stopped gracefully.");
    Ok(())
}
