use crate::{config::load_config_file, restful::ServerAPI};
use cached::instant::Instant;
use clap::Parser;
use ore_api::state::Proof;
use shared::{
    interaction::{BlockHash, Challenge, NextEpoch, User, UserCommand},
    utils::{get_clock, get_latest_blockhash_with_retries, get_updated_proof_with_authority},
};
use solana_program::hash::Hash;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{keypair, Keypair, Signer},
    transaction::Transaction,
};
use std::{error::Error, fs, path::PathBuf, process::exit, sync::Arc, time::Duration};
use tokio::time;
use tracing::*;
use tracing_subscriber::EnvFilter;

mod config;
mod restful;

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

    let mut cfg = load_config_file("./config.json").unwrap();

    debug!("config: {cfg:?}");

    let api = Arc::new(ServerAPI {
        url: format!("http://{}", cfg.server_host),
    });

    let keypairs = parse_keypair()?;

    let keys: Vec<_> = keypairs.iter().map(|m| m.pubkey().to_string()).collect();

    // login and get rpc url
    match api.login("app".to_string(), keys).await {
        Ok((rpc, jito_rpc)) => {
            cfg.rpc = Some(cfg.rpc.unwrap_or(rpc));
            cfg.jito_url = Some(cfg.jito_url.unwrap_or(jito_rpc));
        }
        Err(err) => {
            anyhow::bail!("login error: {err:#}");
        }
    }

    let rpc_client =
        Arc::new(RpcClient::new_with_commitment(cfg.rpc.unwrap(), CommitmentConfig::confirmed()));
    let jito_client = Arc::new(RpcClient::new(cfg.jito_url.unwrap()));

    // create miners
    let miners: Vec<_> = keypairs
        .into_iter()
        .map(|key| {
            Miner {
                keypair: key,
                step: MiningStep::Reset,
                rpc_client: rpc_client.clone(),
                jito_client: jito_client.clone(),
                api: api.clone(),
            }
        })
        .collect();

    // start mining
    let mut handlers = vec![];
    for mut miner in miners {
        let handler = tokio::spawn(async move { miner.run().await });
        handlers.push(handler);
    }

    futures_util::future::join_all(handlers).await;

    Ok(())
}

#[derive(Debug, Clone)]
enum MiningStep {
    Reset,
    Mining,
    Submit,
    Waiting,
}

struct Miner {
    keypair: Keypair,
    step: MiningStep,
    rpc_client: Arc<RpcClient>,
    jito_client: Arc<RpcClient>,
    api: Arc<ServerAPI>,
}

impl Miner {
    async fn run(&mut self) {
        let pubkey = self.keypair.pubkey();
        let mut last_hash_at = 0;
        let mut last_balance = 0;
        let mut deadline = Instant::now();
        loop {
            match self.step {
                MiningStep::Reset => {
                    let proof = get_updated_proof_with_authority(
                        &self.rpc_client,
                        self.keypair.pubkey(),
                        last_hash_at,
                    )
                        .await;
                    last_hash_at = proof.last_hash_at;
                    last_balance = proof.balance;
                    let cutoff_time = self.get_cutoff(proof, 8).await;

                    deadline = Instant::now() + Duration::from_secs(cutoff_time);

                    if let Err(err) =
                        self.api.next_epoch(pubkey.to_string(), proof.challenge, cutoff_time).await
                    {
                        error!("[CMD] update new epoch error: {err:#}")
                    } else {
                        let challenge_str = bs58::encode(&proof.challenge).into_string();
                        info!(
                            "[CMD] {} new epoch: {challenge_str} [{cutoff_time}]",
                            self.keypair.pubkey()
                        );
                        self.step = MiningStep::Mining;
                    }

                    if cutoff_time == 0 {
                        let data = vec![self.keypair.pubkey().to_string()];
                        for i in 0..60 {
                            info!("[CMD] inactive({}) peeking difficulty({i})", self.keypair.pubkey());
                            if let Ok(resp) = self.api.peek_difficulty(data.clone()).await {
                                debug!("peek result: {resp:?}");
                                if resp[0].ge(&8) {
                                    self.step = MiningStep::Submit;
                                    break;
                                }
                            }
                            time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }

                MiningStep::Mining => {
                    if deadline.gt(&Instant::now()) {
                        time::sleep(Duration::from_secs(1)).await;
                    } else {
                        self.step = MiningStep::Submit;
                    }
                }

                MiningStep::Submit => {
                    // let (hash, _slot) = get_latest_blockhash_with_retries(&self.rpc_client)
                    //     .await
                    //     .expect("fail to get latest blockhash ");

                    let hash = Hash::new(&[0_u8; 32]);

                    match self.api.block_hash(pubkey.to_string(), hash.to_bytes()).await {
                        Ok(mut tx) => {
                            info!("[CMD] {:#} new tx received", self.keypair.pubkey());
                            tx.partial_sign(&[&self.keypair], hash);

                            self.step = MiningStep::Waiting;

                            // TODO: submit transaction
                        }
                        Err(err) => {
                            // usually happens when the miner is inactive.
                            // cutoff time is zero. we will have no time to waiting.
                            error!("fetch transaction error: {err:#}");
                            time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }

                MiningStep::Waiting => {
                    time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn get_cutoff(&self, proof: Proof, buffer_time: u64) -> u64 {
        let clock = get_clock(&self.rpc_client).await;
        proof
            .last_hash_at
            .saturating_add(60)
            .saturating_sub(buffer_time as i64)
            .saturating_sub(clock.unix_timestamp)
            .max(0) as u64
    }
}
