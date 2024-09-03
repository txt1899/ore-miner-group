use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use drillx::Solution;
use ore_api::{
    consts::{BUS_ADDRESSES, BUS_COUNT},
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
        proof_pubkey,
    },
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use tokio::sync::{oneshot, RwLock};
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

impl LastRound {
    pub fn update(mut self) -> Self {
        self.elapsed = self.start.elapsed().as_secs();
        self
    }
}

/// create a round of mining
///
/// find the best solution and push it to the packager.
///
/// wait for the result and proceed to the next round
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
        Self {
            api,
            user,
            miner,
            inactive: true,
            cutoff_time: 0,
            response: None,
            current: LastRound::default(),
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
                "stake: {}, difficulty: {}, income: {}, time: {:.2} second", proof.balance,
                difficulty, income, self.last.elapsed
            );
        }

        let mut cutoff = get_cutoff(client, proof, BUFFER_TIME).await;

        if cutoff >= 5 && cutoff < 10 {
            cutoff = 12
        }

        self.current.hash_at = proof.last_hash_at;
        self.current.balance = proof.balance;
        self.current.challenge = proof.challenge;

        self.inactive = cutoff < 5;
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
            let deadline = self.current.start + Duration::from_secs(self.cutoff_time);
            while Instant::now().le(&deadline) {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        }
        Ok(self)
    }

    async fn solution(mut self) -> anyhow::Result<Self> {
        for _ in 0..5 {
            let resp = self
                .api
                .get_solution(self.user.clone(), self.miner.clone(), self.current.challenge)
                .await;

            match resp {
                Ok(data) => {
                    info!("soluction >> nonce: {}, difficulty: {}", data.nonce, data.difficulty);
                    self.current.difficulty = data.difficulty;
                    self.response.replace(data);
                    // let solution = Solution::new(data.digest, data.nonce.to_le_bytes());
                    return Ok(self);
                }
                Err(err) => {
                    error!("fail to get remote solution ({err})")
                }
            };
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("max retries to get remote solution")
    }

    pub fn transaction(&self, bus: Pubkey, signer: &Keypair, tip: u64) -> Vec<Instruction> {
        let data = self.response.as_ref().unwrap();

        let pubkey = signer.pubkey();
        let solution = Solution::new(data.digest, data.nonce.to_le_bytes());

        let mut ixs = vec![];

        ixs.push(ore_api::instruction::auth(proof_pubkey(pubkey)));
        ixs.push(ore_api::instruction::mine(pubkey, pubkey, bus, solution));

        if tip > 0 {
            info!("{:?}, pay the jito tip: {}", pubkey, tip);
            ixs.push(jito::build_bribe_ix(&pubkey, tip));
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
        let result = match client.get_signature_statuses(&signatures[..]).await {
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
                Some(result)
            }
            // Handle confirmation errors
            Err(err) => {
                error!("{:?}", err.kind());
                None
            }
        };

        if result.is_some() && !result.as_ref().unwrap().is_empty() {
            return result;
        }
    }
    None
}

pub struct JitoBundler {
    user: UserName,
    fee_payer: Option<Arc<Keypair>>,
    keypairs: HashMap<MinerKey, Arc<Keypair>>,
    rpc_client: Arc<RpcClient>,
    rounds: RwLock<HashMap<MinerKey, ChallengeRound>>,
    api: Arc<ServerAPI>,

    min_tip: u64,
    max_tip: u64,
    bundle_buffer: u64,
}

impl JitoBundler {
    pub fn new(
        user: UserName,
        fee_payer: Option<Arc<Keypair>>,
        miners: HashMap<MinerKey, Arc<Keypair>>,
        rpc_client: Arc<RpcClient>,
        api: Arc<ServerAPI>,

        min_tip: u64,
        max_tip: u64,
        bundle_buffer: u64,
    ) -> Self {
        Self {
            user,
            fee_payer,
            keypairs: miners,
            rpc_client,
            rounds: Default::default(),
            api,
            min_tip,
            max_tip,
            bundle_buffer,
        }
    }

    async fn push(self: &Arc<Self>, miner: MinerKey, round: ChallengeRound) {
        if self.keypairs.contains_key(&miner) {
            let mut guard = self.rounds.write().await;
            guard.insert(miner, round);
        }
    }

    pub async fn start_mining(self: &Arc<Self>) {
        for (pubkey, keypair) in self.keypairs.iter() {
            let user = self.user.clone();
            let miner = MinerKey(pubkey.to_string());
            let api = Arc::clone(&self.api);

            let client = Arc::clone(&self.rpc_client);
            let signer = Arc::clone(keypair);
            let this = Arc::clone(self);

            tokio::spawn(async move {
                let mut last_round: Option<LastRound> = None;
                loop {
                    debug!("new challenge round");

                    let (tx, rx) = oneshot::channel();

                    let res = ChallengeRound::new(
                        api.clone(),
                        user.clone(),
                        miner.clone(),
                        tx,
                        last_round.clone(),
                    )
                    .mining(&client, &signer)
                    .await;

                    match res {
                        Ok(round) => {
                            debug!("{} waiting bundle", round.user.as_str());

                            this.push(miner.clone(), round).await;

                            // wait transaction result
                            if let Ok(val) = rx.await {
                                last_round = Some(val)
                            }
                        }
                        Err(err) => {
                            error!("fail to mining ({err})");
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                        }
                    }
                }
            });
        }
    }

    pub async fn watcher(self: &Arc<Self>) {
        loop {
            let keys: Vec<_> = {
                let guard = self.rounds.read().await;
                guard
                    .iter()
                    .filter_map(|(key, value)| {
                        (value.remaining() < self.bundle_buffer as i64).then(|| key.clone())
                    })
                    .collect()
            };

            let (handler, rounds) = if !keys.is_empty() {
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

                    let mut guard = self.rounds.write().await;

                    let last = batch.last().unwrap();

                    // TODO
                    // sort the miners by diffculty
                    // the highest difficulty matches the largest balance

                    for key in batch.iter() {
                        let bus = buses.remove(0);

                        let round = guard.remove(key).unwrap();

                        let signer = self.keypairs.get(key).unwrap();

                        // get jito tips
                        let tips = jito::get_jito_tips().await;

                        // the last transaction will included tips instruction
                        let tip = key
                            .eq(last)
                            .then(|| {
                                let min = self.min_tip.max(tips.p50() + 6);
                                if self.max_tip > 0 {
                                    self.max_tip.min(min)
                                } else {
                                    min
                                }
                            })
                            .unwrap_or_default();

                        cost += FEE_PER_SIGNER + tip;

                        let ixs = round.transaction(bus, signer, tip);

                        let fee_payer = self.fee_payer.as_ref().unwrap_or(&signer);

                        let mut tx = Transaction::new_with_payer(&ixs, Some(&fee_payer.pubkey()));

                        if fee_payer.pubkey() == signer.pubkey() {
                            tx.sign(&[&signer], hash);
                        } else {
                            tx.sign(&[&signer, &fee_payer], hash);
                        }

                        bundle.push(tx);
                        rounds.push(round);
                    }
                }
                info!("current bundle cost: {} SOL", amount_u64_to_string(cost));

                (tokio::spawn(async move { jito::send_bundle(bundle).await }), rounds)
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };

            tokio::spawn({
                let client = Arc::clone(&self.rpc_client);
                async move {
                    match handler.await.unwrap() {
                        Ok((signature, bundle_id)) => {
                            debug!(?bundle_id, ?signature, "bundle sent");

                            if let Some(val) = signature_confirm(&client, vec![signature]).await {
                                if !val.is_empty() {
                                    info!("landed: {:?}", val[0])
                                } else {
                                    warn!("bundle dropped")
                                }
                            }

                            for item in rounds {
                                let _ = item.channel.send(item.current.update());
                            }
                        }
                        Err(err) => {
                            error!("fail to send bundle: {err:#}");
                        }
                    }
                }
            });
        }
    }
}
