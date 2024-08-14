use std::future::Future;

use colored::*;
use drillx::{Hash, Solution};
use ore_api::{
    consts::{BUS_ADDRESSES, BUS_COUNT, EPOCH_DURATION},
    state::{Bus, Config, Proof},
};
use ore_utils::AccountDeserialize;
use rand::Rng;
use solana_program::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use tracing::{error, info, log::debug};

use crate::ore::{send_and_confirm::ComputeBudget, Miner};

use super::utils::{
    amount_u64_to_string,
    get_clock,
    get_config,
    get_proof_with_authority,
    proof_pubkey,
};

impl Miner {
    pub async fn mine<F, Fut>(&self, hasher: F)
    where
        Fut: Future<Output = Option<Solution>> + Sized,
        F: FnOnce(Proof, u64, u32) -> Fut, {
        let signer = self.signer();

        // Start mining loop

        // Fetch proof
        let config = get_config(&self.rpc_client).await;
        let proof = get_proof_with_authority(&self.rpc_client, signer.pubkey()).await;
        info!(
            "质押: {} ORE  乘数: {:12}x",
            amount_u64_to_string(proof.balance),
            calculate_multiplier(proof.balance, config.top_balance)
        );

        // Calculate cutoff time
        let cutoff_time = self.get_cutoff(proof, self.buffer_time).await;

        // Run drillx
        if let Some(solution) = hasher(proof, cutoff_time, config.min_difficulty as u32).await {
            // Build instruction set
            let mut ixs = vec![ore_api::instruction::auth(proof_pubkey(signer.pubkey()))];
            let mut compute_budget = 500_000;

            if self.should_reset(config).await && rand::thread_rng().gen_range(0..100).eq(&0) {
                compute_budget += 100_000;
                ixs.push(ore_api::instruction::reset(signer.pubkey()));
            }

            // Build mine ix
            ixs.push(ore_api::instruction::mine(
                signer.pubkey(),
                signer.pubkey(),
                self.find_bus().await,
                solution,
            ));

            // Submit transaction
            self.send_and_confirm(&ixs, ComputeBudget::Fixed(compute_budget), false).await.ok();
        }
    }

    pub fn check_num_cores(&self, cores: u64) {
        let num_cores = num_cpus::get() as u64;
        if cores.gt(&num_cores) {
            println!(
                "{} Cannot exceeds available cores ({})",
                "WARNING".bold().yellow(),
                num_cores
            );
        }
    }

    async fn should_reset(&self, config: Config) -> bool {
        let clock = get_clock(&self.rpc_client).await;
        config
			.last_reset_at
			.saturating_add(EPOCH_DURATION)
			.saturating_sub(5) // Buffer
			.le(&clock.unix_timestamp)
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

    async fn find_bus(&self) -> Pubkey {
        // Fetch the bus with the largest balance
        if let Ok(accounts) = self.rpc_client.get_multiple_accounts(&BUS_ADDRESSES).await {
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
}

fn calculate_multiplier(balance: u64, top_balance: u64) -> f64 {
    1.0 + (balance as f64 / top_balance as f64).min(1.0f64)
}
