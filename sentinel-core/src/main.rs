use anyhow::Result;
use clap::Parser;
use log::{error, info};
use std::path::PathBuf;
use tokio::signal;
use zbus::connection::Builder;

use sentinel_core::audit::AuditLogger;
use sentinel_core::config::SentinelConfig;
use sentinel_core::dbus::SentinelService;
use sentinel_core::pipeline::{init_onnx_runtime, Models};

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

    // A panic on a worker thread would otherwise leave the process running
    // and holding its bus name while no longer answering — every login would
    // then wait for the PAM timeout. Exit instead: systemd restarts the daemon
    // and, until then, pam_sentinel.so falls back to the password at once.
    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic_hook(info);
        std::process::abort();
    }));

    let args = Args::parse();
    info!("Starting Sentinel Recreated Daemon (sentinel-core)...");

    // 2. Load Configuration
    let config_path = PathBuf::from(&args.config);
    let config = match SentinelConfig::load(&config_path) {
        Ok(config) => {
            info!("Loaded configuration from: {}", config_path.display());
            config
        }
        Err(e) => {
            // Do not guess: built-in defaults could be looser than what the
            // owner configured. Without the daemon, pam_sentinel.so steps
            // aside at once and the password prompt is used.
            error!("{:#}", e);
            error!("Face authentication stays off until the config file is fixed (password login is unaffected).");
            std::process::exit(1);
        }
    };

    let models_dir = PathBuf::from(&args.models_dir);
    info!("ONNX models directory: {}", models_dir.display());

    // 3. Load the models once, before the bus name is claimed: when the
    //    name appears on the bus the daemon is ready to authenticate, and no
    //    login ever pays for model loading. A missing or broken detector or
    //    embedder is fatal; without the bus name pam_sentinel.so simply falls
    //    back to the password.
    if let Err(e) = init_onnx_runtime(config.hardware.onnx_num_threads) {
        error!("{:#}", e);
    }
    let models = match Models::load(&models_dir, &config.detection) {
        Ok(models) => models,
        Err(e) => {
            error!(
                "Could not load ONNX models from '{}': {:#}. Ensure scrfd_500m_kps.onnx and mobile_facenet.onnx are present.",
                models_dir.display(),
                e
            );
            std::process::exit(1);
        }
    };
    if models.spoof.is_none() {
        error!("No anti-spoof model loaded: face authentication will refuse every request (password still works).");
    }
    info!("ONNX models loaded.");

    // 4. Initialize AuditLogger and run 30-day retention cleanup
    info!("Initializing AuditLogger and checking retention policy (30 days)...");
    let audit_logger = AuditLogger::new();
    let _ = audit_logger.cleanup_old_logs(30);

    // 5. Claim the DBus name (systemd Type=dbus waits for it)
    let rt_handle = tokio::runtime::Handle::current();
    let service = SentinelService::new(config, config_path, models_dir.clone(), models, rt_handle);

    info!("Registering DBus service on System Bus under 'com.sentinel.Sentinel'...");
    let _conn = Builder::system()?
        .name("com.sentinel.Sentinel")?
        .serve_at("/com/sentinel/Sentinel", service)?
        .build()
        .await?;

    info!("Successfully claimed System DBus name 'com.sentinel.Sentinel'.");

    // 6. Graceful Shutdown Handler (SIGINT / SIGTERM)
    info!("Daemon running. Awaiting shutdown signals (SIGINT / SIGTERM)...");
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }
    info!("Received shutdown signal. Exiting...");

    info!("Sentinel Daemon stopped gracefully.");
    Ok(())
}
