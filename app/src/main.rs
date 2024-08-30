use anyhow::anyhow;
use cached::instant::Instant;
use clap::Parser;
use colored::Colorize;
use drillx::Solution;
use ore_api::{error::OreError, state::Proof};
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::{
    hash::Hash,
    native_token::lamports_to_sol,
    pubkey::Pubkey,
    system_instruction::transfer,
};
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    compute_budget::ComputeBudgetInstruction,
    signature::{keypair, Keypair, Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};
use std::{
    cmp::min,
    error::Error,
    fs,
    path::PathBuf,
    process::exit,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::time;
use tracing::*;
use tracing_subscriber::EnvFilter;

use crate::{config::load_config_file, restful::ServerAPI};
use shared::{
    interaction::{NextEpoch, Peek, User},
    jito,
    types::{MinerKey, UserName},
    utils::{
        get_clock,
        get_latest_blockhash_with_retries,
        get_updated_proof_with_authority,
        proof_pubkey,
    },
};

mod bet_bus;
mod config;
mod dynamic_fee;
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

    #[arg(long, help = "Enable dynamic priority fees", global = true)]
    dynamic_fee: bool,

    #[arg(long, help = "Add jito tip to the miner. Defaults to false", global = true)]
    jito: bool,
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

    let mut cfg = load_config_file("./config.json")?;

    debug!("config: {cfg:?}");

    let api = Arc::new(ServerAPI {
        url: format!("http://{}", cfg.server_host),
    });

    let keys = parse_keypair()?;

    let user_name = UserName(cfg.user.clone());

    let rpc_client =
        Arc::new(RpcClient::new_with_commitment(cfg.rpc.unwrap(), CommitmentConfig::confirmed()));

    let jito_client = Arc::new(RpcClient::new(cfg.jito_url.unwrap()));

    let payer = match cfg.fee_payer {
        None => None,
        Some(path) => {
            Some(Arc::new(
                keypair::read_keypair_file(path)
                    .map_err(|_| anyhow::anyhow!("not keypair found"))?,
            ))
        }
    };

    let miner_keys: Vec<_> = keys.iter().map(|m| MinerKey(m.pubkey().to_string())).collect();
    // login and get rpc url
    match api.login(user_name.clone(), miner_keys).await {
        Ok((rpc, jito_rpc)) => {
            //cfg.rpc = Some(cfg.rpc.unwrap_or(rpc));
            // cfg.jito_url = Some(cfg.jito_url.unwrap_or(jito_rpc));
        }
        Err(err) => {
            anyhow::bail!("login error: {err:#}");
        }
    }
    // create miners
    let miners: Vec<_> = keys
        .into_iter()
        .map(|key| {
            Miner {
                user: user_name.clone(),
                signer: key,
                payer: payer.clone(),
                step: MiningStep::Reset,
                rpc_client: rpc_client.clone(),
                jito_client: jito_client.clone(),
                priority_fee: args.priority_fee,
                dynamic_fee: args.dynamic_fee,
                dynamic_fee_url: cfg.dynamic_fee_url.clone(),
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
    user: UserName,
    signer: Keypair,
    payer: Option<Arc<Keypair>>,
    step: MiningStep,
    rpc_client: Arc<RpcClient>,
    jito_client: Arc<RpcClient>,
    priority_fee: Option<u64>,
    dynamic_fee: bool,
    dynamic_fee_url: Option<String>,
    api: Arc<ServerAPI>,
}

const CONFIRM_RETRIES: u64 = 10;
const CONFIRM_DELAY: u64 = 500;
const GATEWAY_RETRIES: u64 = 5;

impl Miner {
    async fn run(&mut self) {
        let pubkey = self.signer.pubkey().to_string();
        let mut last_hash_at = 0;
        let mut last_balance = 0;
        let mut deadline = Instant::now();
        loop {
            match self.step {
                MiningStep::Reset => {
                    let proof = get_updated_proof_with_authority(
                        &self.rpc_client,
                        self.signer.pubkey(),
                        last_hash_at,
                    )
                    .await;

                    last_hash_at = proof.last_hash_at;
                    last_balance = proof.balance;

                    let cutoff_time = self.get_cutoff(proof, 8).await;

                    deadline = Instant::now() + Duration::from_secs(cutoff_time);

                    if let Err(err) = self
                        .api
                        .next_epoch(
                            self.user.clone(),
                            MinerKey(pubkey.clone()),
                            proof.challenge,
                            cutoff_time,
                        )
                        .await
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
                        let data = Peek {
                            user: self.user.clone(),
                            miners: vec![MinerKey(pubkey.clone())],
                        };
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
                    match self.api.get_solution(self.user.clone(), MinerKey(pubkey.clone())).await {
                        Ok(data) => {
                            let s = Solution::new(data.digest, data.nonce.to_le_bytes());

                            if let Err(err) = self.transaction(s, MinerKey(pubkey.clone())).await {
                                error!("{} >> max retries: {:?}", pubkey, err);
                                self.step = MiningStep::Reset;
                                continue;
                            }
                        }
                        Err(err) => {
                            error!("fail to get solution: {err:?}");
                            time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }

                MiningStep::Waiting => {
                    time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn transaction(&self, solution: Solution, miner: MinerKey) -> anyhow::Result<Signature> {
        info!("{} >> ready for transaction", self.signer.pubkey());

        anyhow::bail!("test err");

        let compute_budget = 500_000;
        let pubkey = Pubkey::from_str(miner.as_str())?;
        let mut client = self.rpc_client.clone();
        let mut sigs = vec![];

        let payer = match self.payer {
            None => &self.signer,
            Some(ref payer) => payer,
        };

        let mut final_ixs = vec![];

        // ix: 0: set compute unit limit
        final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(compute_budget));

        // ix: 1: set compute unit price, we will set the true value later.
        final_ixs.push(ComputeBudgetInstruction::set_compute_unit_price(0));

        // ix: 2: ore auth instruction
        final_ixs.push(ore_api::instruction::auth(proof_pubkey(pubkey)));

        // ix: 3: ore mine instruction
        final_ixs.push(ore_api::instruction::mine(pubkey, pubkey, self.find_bus().await, solution));

        let tip = jito::get_jito_tips().await;
        let tip_value = tip.p50();
        if tip_value > 0 {
            client = self.jito_client.clone();
            info!("jito tip value: {} SOL", lamports_to_sol(tip_value));
            let tip_account = jito::pick_jito_recipient();

            // ix: 4: transfer tip to jito
            final_ixs.push(transfer(&payer.pubkey(), tip_account, tip.p50()));
        }

        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(0),
            min_context_slot: None,
        };

        let mut attempts = 0;

        loop {
            // get real gas
            let real_fee = if self.dynamic_fee {
                match self.dynamic_fee().await {
                    Ok(fee) => fee,
                    Err(err) => {
                        let fee = self.priority_fee.unwrap_or(0);
                        info!(
                            "  {} {} use fixed priority fee: {}",
                            "WARNING".bold().yellow(),
                            err,
                            fee
                        );
                        fee
                    }
                }
            } else {
                self.priority_fee.unwrap_or(0)
            };

            // set real priority fee
            final_ixs.remove(1);
            final_ixs.insert(1, ComputeBudgetInstruction::set_compute_unit_price(real_fee));

            info!("priority fee: {} SOL", lamports_to_sol(real_fee));

            let (hash, _slot) = get_latest_blockhash_with_retries(&self.rpc_client)
                .await
                .expect("fail to get latest blockhash");

            let mut tx = Transaction::new_with_payer(&final_ixs, Some(&payer.pubkey()));

            if self.signer.pubkey() == payer.pubkey() {
                tx.sign(&[&self.signer], hash);
            } else {
                tx.sign(&[&self.signer, payer], hash);
            }

            let sig = client.send_transaction_with_config(&tx, send_cfg).await?;
            sigs.push(sig);

            attempts += 1;
            // Send transaction
            if let Some(sig) = self.send_confirm(&self.rpc_client, &mut sigs).await? {
                return Ok(sig);
            }

            if attempts > GATEWAY_RETRIES {
                error!("max retries");
                anyhow::bail!("max retries ")
            }
        }
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
