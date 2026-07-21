// you don't want your application runtime to randomly crash
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::panic)]
#![warn(clippy::todo)]

use {
    std::path::PathBuf,
    tracing::{info, warn, error},
    tracing_subscriber::{FmtSubscriber, EnvFilter},
    clap::{Parser, Subcommand},
    fx_runtime::{FxServer, config::ServerConfig},
};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(long, action)]
    debug: Option<bool>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        config_file: String,
    },
}

#[cfg(not(target_arch = "aarch64"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let args = Args::parse();
    let env_filter = if args.debug.unwrap_or(false) {
        EnvFilter::new("fx_runtime=debug,info")
    } else {
        EnvFilter::new("info")
    };
    FmtSubscriber::builder()
        .with_env_filter(env_filter)
        .init();

    match args.command {
        Command::Serve { config_file } => {
            let current_dir = match std::env::current_dir() {
                Ok(v) => v,
                Err(err) => {
                    let default_work_dir = PathBuf::from("/var/lib/fx");
                    warn!("failed to get workdir: {err:?}. Defaulting to {default_work_dir:?}");
                    default_work_dir
                }
            };
            let config_path = current_dir.join(config_file);

            info!("Loading config from {config_path:?}");
            let config = ServerConfig::load(config_path);

            let server = match FxServer::new(config).start() {
                Ok(v) => v,
                Err(err) => {
                    error!("failed to start fx server: {err:?}. Exiting...");
                    return;
                }
            };

            server.wait_until_finished();
        },
    }
}
