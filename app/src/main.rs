use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

use bundler::JitoBundler;
use clap::Parser;
use shared::{
    jito,
    types::{MinerKey, UserName},
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{keypair, Keypair, Signer},
};
use tracing::*;
use tracing_subscriber::EnvFilter;

use crate::{config::load_config_file, restful::ServerAPI};

mod bundler;
mod config;
mod restful;

#[derive(Parser, Debug)]
#[command(about, version)]
struct Args {
    #[arg(
        long,
        value_name = "MICROLAMPORTS",
        help = "Price to pay for compute unit. If dynamic fee url is also set, this value will be the max.",
        default_value = "100000",
        global = true
    )]
    priority_fee: Option<u64>,

    // #[arg(long, help = "Enable dynamic priority fees", global = true)]
    // dynamic_fee: bool,
    #[arg(long, help = "The min tip to pay for jito.", default_value = "1000", global = true)]
    min_tip: Option<u64>,

    #[arg(
        long,
        help = "The max tip to pay for jito. Set to 0 to disable adaptive tip.",
        default_value = "0",
        global = true
    )]
    max_tip: Option<u64>,

    #[arg(
        long,
        help = "Collect bundle transaction buffer time.",
        default_value = "5",
        global = true
    )]
    bundle_buffer: Option<u64>,
}

fn init_log() {
    let env_filter = EnvFilter::from_default_env()
        .add_directive("app=trace".parse().unwrap())
        .add_directive("info".parse().unwrap());
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

fn read_keypair_file() -> std::io::Result<Vec<PathBuf>> {
    let dir = "./account";
    let mut files = vec![];
    let items = fs::read_dir(dir)?;
    for item in items {
        let e = item?;
        let path = e.path();
        if path.is_file() {
            if let Some(e) = path.extension() {
                if e == "json" {
                    files.push(path);
                }
            }
        }
    }
    Ok(files)
}

fn parse_keypair() -> anyhow::Result<Vec<Keypair>> {
    let paths = match read_keypair_file() {
        Ok(paths) => paths,
        Err(err) => {
            anyhow::bail!("read keypair error: {err:#}");
        }
    };

    paths
        .iter()
        .map(|path| {
            match keypair::read_keypair_file(path.clone()) {
                Ok(key) => Ok(key),
                Err(_) => {
                    anyhow::bail!("read keypair file: {path:?}");
                }
            }
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_log();

    let args = Args::parse();

    let cfg = load_config_file("./config.json")?;

    debug!("config: {cfg:?}");

    let api = Arc::new(ServerAPI {
        url: format!("http://{}", cfg.server_host),
    });

    let keys = parse_keypair()?;

    let user_name = UserName(cfg.user.clone());

    let rpc_client =
        Arc::new(RpcClient::new_with_commitment(cfg.rpc.unwrap(), CommitmentConfig::confirmed()));

    let fee_payer = match cfg.fee_payer {
        None => None,
        Some(path) => {
            Some(Arc::new(
                keypair::read_keypair_file(path)
                    .map_err(|_| anyhow::anyhow!("not keypair found"))?,
            ))
        }
    };

    let miner_keys: Vec<_> = keys.iter().map(|m| MinerKey(m.pubkey().to_string())).collect();

    tokio::spawn(async { jito::subscribe_jito_tips().await });

    // login
    if let Err(err) = api.login(user_name.clone(), miner_keys).await {
        anyhow::bail!("fail to login, not match miners key: {err:#}");
    }
    // create miners
    let miners: HashMap<_, _> =
        keys.into_iter().map(|key| (MinerKey(key.pubkey().to_string()), Arc::new(key))).collect();

    let jito_bundler = Arc::new(JitoBundler::new(
        user_name,
        fee_payer,
        miners,
        rpc_client,
        api,
        args.min_tip.unwrap_or(1000),
        args.max_tip.unwrap_or_default(),
        args.bundle_buffer.unwrap_or_default(),
    ));

    jito_bundler.start_mining().await;

    jito_bundler.watcher().await;

    Ok(())
}
