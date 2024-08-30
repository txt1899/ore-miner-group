use std::{error::Error, fs, path::PathBuf, process::exit, sync::Arc, time::Duration};

use cached::instant::Instant;
use clap::Parser;
use ore_api::{error::OreError, state::Proof};
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::hash::Hash;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    signature::{keypair, Keypair, Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};
use tokio::time;
use tracing::*;
use tracing_subscriber::EnvFilter;

use shared::{
    interaction::{NextEpoch, User, UserCommand},
    utils::{get_clock, get_latest_blockhash_with_retries, get_updated_proof_with_authority},
};
use shared::types::MinerKey;

use crate::{config::load_config_file, restful::ServerAPI};

mod config;
mod restful;

fn init_log() {
    let env_filter = EnvFilter::from_default_env()
        //.add_directive("app=trace".parse().unwrap())
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
        user: cfg.user,
        url: format!("http://{}", cfg.server_host),
    });

    let keypairs = parse_keypair()?;

    let keys: Vec<_> = keypairs.iter().map(|m| MinerKey(m.pubkey().to_string())).collect();

    // login and get rpc url
    match api.login(keys).await {
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

const CONFIRM_RETRIES: u64 = 10;
const CONFIRM_DELAY: u64 = 500;
const GATEWAY_RETRIES: u64 = 5;

impl Miner {
    async fn run(&mut self) {
        let pubkey = self.keypair.pubkey().to_string();
        let mut last_hash_at = 0;
        let mut last_balance = 0;
        let mut deadline = Instant::now();
        let mut sigs = vec![];
        let mut attempts = 0;
        loop {
            match self.step {
                MiningStep::Reset => {
                    let proof = get_updated_proof_with_authority(
                        &self.rpc_client,
                        self.keypair.pubkey(),
                        last_hash_at,
                    )
                        .await;

                    sigs = vec![];
                    attempts = 0;

                    last_hash_at = proof.last_hash_at;
                    last_balance = proof.balance;

                    let cutoff_time = self.get_cutoff(proof, 8).await;

                    deadline = Instant::now() + Duration::from_secs(cutoff_time);

                    if let Err(err) =
                        self.api.next_epoch(MinerKey(pubkey.clone()), proof.challenge, cutoff_time).await
                    {
                        error!("update new epoch error: {err:#}")
                    } else {
                        let challenge_str = bs58::encode(&proof.challenge).into_string();
                        info!("{} >> challenge: {challenge_str} cutoff: {cutoff_time}", pubkey);
                        self.step = MiningStep::Mining;
                    }

                    // miner is inactive. we should peek the difficulty.
                    // if the difficulty is higher than 8.
                    // we should submit a transaction to activate the miner.
                    if cutoff_time == 0 {
                        let data = vec![pubkey.clone()];
                        for i in 0..60 {
                            info!("{} >> peeking difficulty({i})", pubkey);
                            if let Ok(resp) = self.api.peek_difficulty(data.clone()).await {
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
                    if attempts > GATEWAY_RETRIES {
                        error!("{} >> Max retries", pubkey);
                        self.step = MiningStep::Reset;
                        continue;
                    }

                    let (hash, _slot) = get_latest_blockhash_with_retries(&self.rpc_client)
                        .await
                        .expect("fail to get latest blockhash");

                    match self.send_transaction(&mut sigs, MinerKey(pubkey.clone()), hash).await {
                        Ok(r) => {
                            if let Some(sig) = r {
                                info!("{} >> {} {}", self.keypair.pubkey(), "OK", sig);
                                self.step = MiningStep::Reset;
                            }
                        }
                        Err(err) => {
                            error!("{} >> submit error: {err:#}", pubkey);
                        }
                    }
                    attempts += 1;
                }

                MiningStep::Waiting => {
                    time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn send_transaction(
        &self,
        sigs: &mut Vec<Signature>,
        miner: MinerKey,
        hash: Hash,
    ) -> anyhow::Result<Option<Signature>> {
        // let mut resp = self.api.block_hash(miner, hash.to_bytes()).await?;
        //
        // info!("{} >> ready for transaction", self.keypair.pubkey());
        //
        // resp.tx.partial_sign(&[&self.keypair], hash);
        //
        // let rpc = if resp.jito {
        //     self.jito_client.clone()
        // } else {
        //     self.rpc_client.clone()
        // };
        //
        // let send_cfg = RpcSendTransactionConfig {
        //     skip_preflight: true,
        //     preflight_commitment: Some(CommitmentLevel::Confirmed),
        //     encoding: Some(UiTransactionEncoding::Base64),
        //     max_retries: Some(0),
        //     min_context_slot: None,
        // };
        //
        // let sig = rpc.send_transaction_with_config(&resp.tx, send_cfg).await?;
        //
        // sigs.push(sig);
        //
        // // Send transaction
        // if let Some(sig) = self.send_confirm(&self.rpc_client, sigs).await? {
        //     return Ok(Some(sig));
        // }
        Ok(None)
    }

    async fn send_confirm(
        &self,
        client: &Arc<RpcClient>,
        sigs: &mut Vec<Signature>,
    ) -> ClientResult<Option<Signature>> {
        'confirm: for _ in 0..CONFIRM_RETRIES {
            tokio::time::sleep(Duration::from_millis(CONFIRM_DELAY)).await;
            match client.get_signature_statuses(&sigs[..]).await {
                Ok(signature_statuses) => {
                    for (index, status) in signature_statuses.value.iter().enumerate() {
                        if let Some(status) = status {
                            if let Some(ref err) = status.err {
                                match err {
                                    // Instruction error
                                    solana_sdk::transaction::TransactionError::InstructionError(
                                        _,
                                        err,
                                    ) => {
                                        match err {
                                            // Custom instruction error, parse into OreError
                                            solana_program::instruction::InstructionError::Custom(err_code) => {
                                                match err_code {
                                                    e if (OreError::NeedsReset as u32).eq(e) => {
                                                        sigs.remove(index);
                                                        error!( "Needs reset. retry...");
                                                        break 'confirm;
                                                    }
                                                    _ => {
                                                        error!("{err:?}");
                                                        return Err(ClientError {
                                                            request: None,
                                                            kind: ClientErrorKind::Custom(err.to_string()),
                                                        });
                                                    }
                                                }
                                            }

                                            // Non custom instruction error, return
                                            _ => {
                                                error!("{err:?}");
                                                return Err(ClientError {
                                                    request: None,
                                                    kind: ClientErrorKind::Custom(err.to_string()),
                                                });
                                            }
                                        }
                                    }

                                    // Non instruction error, return
                                    _ => {
                                        error!("{err:?}");
                                        return Err(ClientError {
                                            request: None,
                                            kind: ClientErrorKind::Custom(err.to_string()),
                                        });
                                    }
                                }
                            } else if let Some(ref confirmation) = status.confirmation_status {
                                debug!("confirmation: {:?}", confirmation);
                                match confirmation {
                                    TransactionConfirmationStatus::Processed => {}
                                    TransactionConfirmationStatus::Confirmed
                                    | TransactionConfirmationStatus::Finalized => {
                                        return Ok(Some(sigs[index]));
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle confirmation errors
                Err(err) => {
                    error!("{:?}", err.kind());
                }
            }
        }
        Ok(None)
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
