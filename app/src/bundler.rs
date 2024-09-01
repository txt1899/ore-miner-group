use std::{
    cmp::min,
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use drillx::Solution;
use ore_api::{
    consts::{BUS_ADDRESSES, BUS_COUNT},
    error::OreError,
    state::{Bus, Proof},
};
use ore_utils::AccountDeserialize;
use rand::Rng;
use shared::{
    interaction::{Peek, SolutionResponse},
    jito::{self, FEE_PER_SIGNER},
    types::{MinerKey, UserName},
    utils::{
        amount_u64_to_string,
        get_clock,
        get_latest_blockhash_with_retries,
        get_updated_proof_with_authority,
    },
};
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    nonblocking::rpc_client::RpcClient,
};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use solana_transaction_status::TransactionConfirmationStatus;
use tokio::sync::{oneshot, Mutex};
use tracing::*;

use crate::restful::ServerAPI;

const BUFFER_TIME: u64 = 10;
const CONFIRM_RETRIES: u64 = 15;
const CONFIRM_DELAY: u64 = 500;

#[derive(Clone)]
pub struct LastRound {
    start: Instant,
    difficulty: u32,
    balance: u64,
    challenge: [u8; 32],
    elapsed: u64,
    hash_at: i64,
}

impl Default for LastRound {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            difficulty: 0,
            balance: 0,
            challenge: [0u8; 32],
            elapsed: 0,
            hash_at: 0,
        }
    }
}

pub struct ChallengeRound {
    api: Arc<ServerAPI>,
    user: UserName,
    miner: MinerKey,
    inactive: bool,
    cutoff_time: u64,
    response: Option<SolutionResponse>,
    current: LastRound,
    last: LastRound,

    channel: oneshot::Sender<LastRound>,
}

impl ChallengeRound {
    pub fn new(
        api: Arc<ServerAPI>,
        user: UserName,
        miner: MinerKey,
        channel: oneshot::Sender<LastRound>,
        last: Option<LastRound>,
    ) -> Self {
        let current = LastRound::default();
        Self {
            api,
            user,
            miner,
            inactive: true,
            cutoff_time: 0,
            response: None,
            current,
            last: last.unwrap_or_default(),
            channel,
        }
    }

    pub fn remaining(&self) -> i64 {
        let elapsed = self.current.start.elapsed().as_secs();
        let max = self.cutoff_time + BUFFER_TIME;
        max as i64 - elapsed as i64
    }

    pub async fn mining(self, client: &RpcClient, signer: &Keypair) -> anyhow::Result<Self> {
        self.step_initialize(client, signer).await?.step_mining().await?.solution().await
    }

    async fn step_initialize(
        mut self,
        client: &RpcClient,
        signer: &Keypair,
    ) -> anyhow::Result<Self> {
        let LastRound {
            difficulty,
            balance,
            hash_at,
            ..
        } = self.last;

        let proof = get_updated_proof_with_authority(client, signer.pubkey(), hash_at).await?;

        if difficulty > 0 {
            let income = amount_u64_to_string(proof.balance.saturating_sub(balance));
            info!(
                "difficulty: {}, income: {}, time: {:.2} second",
                difficulty, income, self.last.elapsed
            );
        }

        let cutoff = get_cutoff(client, proof, BUFFER_TIME).await;

        self.current.hash_at = proof.last_hash_at;
        self.current.balance = proof.balance;
        self.current.challenge = proof.challenge;

        self.inactive = cutoff == 0;
        self.cutoff_time = cutoff;

        self.api
            .next_epoch(self.user.clone(), self.miner.clone(), self.current.challenge, cutoff)
            .await?;

        Ok(self)
    }

    async fn step_mining(self) -> anyhow::Result<Self> {
        if self.inactive {
            let data = Peek {
                user: self.user.clone(),
                miners: vec![self.miner.clone()],
            };
            for i in 0..60 {
                info!("{} >> peeking difficulty({i})", self.miner.as_str());
                if let Ok(resp) = self.api.peek_difficulty(data.clone()).await {
                    if resp[0].ge(&8) {
                        return Ok(self);
                    }
                }
                tokio::time::sleep(Duration::from_millis(3000)).await;
            }
            anyhow::bail!("activate miner timeout")
        } else {
            let deadline = Instant::now() + Duration::from_secs(self.cutoff_time);
            while Instant::now().le(&deadline) {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        }
        Ok(self)
    }

    async fn solution(mut self) -> anyhow::Result<Self> {
        for i in 0..10 {
            let resp = self
                .api
                .get_solution(self.user.clone(), self.miner.clone(), self.current.challenge)
                .await;

            match resp {
                Ok(data) => {
                    self.current.difficulty = data.difficulty;
                    self.response.replace(data);
                    // let solution = Solution::new(data.digest, data.nonce.to_le_bytes());
                    break;
                }
                Err(err) => {
                    error!("fail to get remote solution ({err})")
                }
            };
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        Ok(self)
    }

    pub fn transaction(&self, bus: Pubkey, signer: &Keypair, tips: u64) -> Vec<Instruction> {
        let data = self.response.as_ref().unwrap();
        let pubkey = signer.pubkey();
        let compute_budget = 500_000;
        let solution = Solution::new(data.digest, data.nonce.to_le_bytes());

        let mut ixs = vec![];

        ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(compute_budget));
        ixs.push(ore_api::instruction::mine(pubkey, pubkey, bus, solution));

        if tips > 0 {
            ixs.push(jito::build_bribe_ix(&pubkey, tips));
        }

        ixs
    }
}

async fn get_cutoff(client: &RpcClient, proof: Proof, buffer_time: u64) -> u64 {
    let clock = get_clock(client).await.expect("get solana clock max retries");
    proof
        .last_hash_at
        .saturating_add(60)
        .saturating_sub(buffer_time as i64)
        .saturating_sub(clock.unix_timestamp)
        .max(0) as u64
}

pub async fn find_bus(client: &RpcClient) -> Pubkey {
    // Fetch the bus with the largest balance
    if let Ok(accounts) = client.get_multiple_accounts(&BUS_ADDRESSES).await {
        let mut top_bus_balance: u64 = 0;
        let mut top_bus = BUS_ADDRESSES[0];
        for account in accounts {
            if let Some(account) = account {
                if let Ok(bus) = Bus::try_from_bytes(&account.data) {
                    if bus.rewards.gt(&top_bus_balance) {
                        top_bus_balance = bus.rewards;
                        top_bus = BUS_ADDRESSES[bus.id as usize];
                    }
                }
            }
        }
        return top_bus;
    }

    // Otherwise return a random bus
    let i = rand::thread_rng().gen_range(0..BUS_COUNT);
    BUS_ADDRESSES[i]
}

pub async fn get_buses(client: &RpcClient) -> Vec<Pubkey> {
    let mut buses = vec![];
    if let Ok(accounts) = client.get_multiple_accounts(&BUS_ADDRESSES).await {
        for account in accounts {
            if let Some(account) = account {
                if let Ok(bus) = Bus::try_from_bytes(&account.data) {
                    buses.push(bus.clone())
                }
            }
        }
        buses.sort_by(|a, b| b.rewards.cmp(&a.rewards));

        let resp: Vec<_> = buses.into_iter().map(|item| BUS_ADDRESSES[item.id as usize]).collect();
        return resp;
    }

    BUS_ADDRESSES.to_vec()
}

async fn signature_confirm(
    client: &Arc<RpcClient>,
    signatures: Vec<Signature>,
) -> Option<Vec<Signature>> {
    for _ in 0..CONFIRM_RETRIES {
        tokio::time::sleep(Duration::from_millis(CONFIRM_DELAY)).await;
        match client.get_signature_statuses(&signatures[..]).await {
            Ok(signature_statuses) => {
                let result = signature_statuses
                    .value
                    .into_iter()
                    .zip(signatures.iter())
                    .filter_map(|(status, sig)| {
                        if status?.satisfies_commitment(CommitmentConfig::confirmed()) {
                            Some(*sig)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                return Some(result);
            }
            // Handle confirmation errors
            Err(err) => {
                error!("{:?}", err.kind());
            }
        }
    }
    None
}

pub struct JitoBundler {
    user: UserName,
    keypairs: HashMap<String, Arc<Keypair>>,
    rpc_client: Arc<RpcClient>,
    rounds: Mutex<HashMap<String, ChallengeRound>>,
    api: Arc<ServerAPI>,
}

impl JitoBundler {
    pub async fn run(self: &Arc<Self>) {
        for (pubkey, keypair) in self.keypairs.iter() {
            let user = self.user.clone();
            let miner = MinerKey(pubkey.to_string());
            let api = Arc::clone(&self.api);

            let client = Arc::clone(&self.rpc_client);
            let signer = Arc::clone(keypair);
            let this = Arc::clone(self);

            tokio::spawn(async move {
                let mut last_rund: Option<LastRound> = None;
                loop {
                    let (tx, rx) = oneshot::channel();

                    let res = ChallengeRound::new(
                        api.clone(),
                        user.clone(),
                        miner.clone(),
                        tx,
                        last_rund.clone(),
                    )
                    .mining(&client, &signer)
                    .await;

                    match res {
                        Ok(round) => {
                            this.push(miner.as_str().to_string(), round).await;
                            if let Ok(val) = rx.await {
                                last_rund = Some(val)
                            }
                        }
                        Err(err) => {
                            error!("fail to minig ({err})")
                        }
                    }
                }
            });
        }
    }

    async fn push(self: &Arc<Self>, pubkey: String, round: ChallengeRound) {
        if let Some(_) = self.keypairs.get(&pubkey) {
            let mut guard = self.rounds.lock().await;
            guard.insert(pubkey, round);
        }
    }

    pub async fn watch(&self) {
        loop {
            let mut guard = self.rounds.lock().await;

            let keys: Vec<_> = guard
                .iter()
                .filter_map(|(key, value)| (value.remaining() < 5).then(|| key.clone()))
                .collect();

            let task = if !keys.is_empty() {
                let mut cost = 0_u64;
                let mut bundle = Vec::with_capacity(5);
                let mut rounds = Vec::with_capacity(5);

                // get up to 5 wallets
                for batch in keys.chunks(5) {
                    // sort the bus account by the balance
                    let mut buses = get_buses(&self.rpc_client).await;

                    let (hash, _) = match get_latest_blockhash_with_retries(&self.rpc_client).await
                    {
                        Ok(val) => val,
                        Err(err) => {
                            error!("fail to get latest blockhash ({err})");
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                            continue;
                        }
                    };

                    let last = batch.last().unwrap();
                    for key in batch.iter() {
                        let bus = buses.remove(0);

                        let round = guard.remove(key).unwrap();

                        let signer = self.keypairs.get(key).unwrap();

                        // get jito tips
                        let tips = jito::get_jito_tips().await;

                        // the last transaction will included tips instruction
                        let tip = key.eq(last).then(|| min(tips.p50(), 10000)).unwrap_or_default();

                        cost += FEE_PER_SIGNER + tip;

                        let ixs = round.transaction(bus, signer, tip);
                        let mut tx = Transaction::new_with_payer(&ixs, Some(&signer.pubkey()));
                        tx.sign(&[&signer], hash);

                        bundle.push(tx);
                        rounds.push(round);
                    }
                }
                Some((tokio::spawn(async move { jito::send_bundle(bundle).await }), rounds))
            } else {
                None
            };

            // if there is a jito transaction, check the result
            if let Some(mut val) = task {
                let client = Arc::clone(&self.rpc_client);
                tokio::spawn(async move {
                    match val.0.await.unwrap() {
                        Ok((signature, bundle_id)) => {
                            debug!(?bundle_id, ?signature, "bundle sent");

                            if let Some(val) = signature_confirm(&client, vec![signature]).await {
                                if !val.is_empty() {
                                    error!("landed: {:?}", val[0])
                                } else {
                                    warn!("bundle dropped")
                                }
                            }

                            for item in val.1 {
                                let _ = item.channel.send(item.current);
                            }
                        }
                        Err(err) => {
                            error!("fail to send bundle: {err:#}");
                        }
                    }
                });
            }
        }
    }
}
