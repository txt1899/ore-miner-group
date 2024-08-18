use actix::Addr;
use chrono::Local;
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
use tokio::time::{sleep, Instant};
use tracing::{error, info, log::debug};

use crate::{
    ore::{send_and_confirm::ComputeBudget, Miner},
    websocket::{jito::JitoActor, messages},
};

use super::utils::{
    amount_u64_to_string,
    get_clock,
    get_config,
    get_proof_with_authority,
    proof_pubkey,
};

impl Miner {
    pub async fn mine<F, Fut>(&self, jito: Addr<JitoActor>, hasher: F)
    where
        Fut: Future<Output = Option<(u32, Solution)>> + Sized,
        F: FnOnce(Proof, u64, u32) -> Fut, {
        let signer = self.signer();

        // Start mining loop

        // Fetch proof
        let config = get_config(&self.rpc_client).await.expect("获取配置失败");
        let proof = get_proof_with_authority(&self.rpc_client, signer.pubkey())
            .await
            .expect("获取Proof信息失败");

        // Calculate cutoff time
        let cutoff_time = self.get_cutoff(proof, self.buffer_time).await;

        let start_hash = Instant::now();
        // Run drillx
        if let Some((difficulty, solution)) =
            hasher(proof, cutoff_time, config.min_difficulty as u32).await
        {
            // 挖矿耗时
            let hash_elapsed = start_hash.elapsed();

            // Build instruction set
            let mut ixs = vec![ore_api::instruction::auth(proof_pubkey(signer.pubkey()))];
            let mut compute_budget = 480_000;

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

            let value = jito.send(messages::WithTip).await.expect("获取jito小费失败");

            let start_submit = Instant::now();
            // Submit transaction
            if let Ok(tx) = self
                .send_and_confirm(&ixs, ComputeBudget::Fixed(compute_budget), false, value, Some(difficulty))
                .await
            {
                // 提交耗时
                let submit_elapsed = start_submit.elapsed();

                // 尝试等待200ms，更新Proof
                sleep(std::time::Duration::from_millis(200)).await;

                let new_proof = get_proof_with_authority(&self.rpc_client, signer.pubkey())
                    .await
                    .expect("获取Proof信息失败");

                let earned = amount_u64_to_string(new_proof.balance.saturating_sub(proof.balance));

                info!(
                    "质押: {} ORE  乘数: {:12}x",
                    amount_u64_to_string(new_proof.balance),
                    calculate_multiplier(new_proof.balance, config.top_balance)
                );
                info!(
                    "难度: {difficulty}, 收益: {}, 挖矿耗时: {:.2}秒, 提交耗时: {:.2}秒, 总耗时: {:.2}秒",
                    earned.bold().green(),
                    hash_elapsed.as_secs_f32(),
                    submit_elapsed.as_secs_f32(),
                    (hash_elapsed.as_secs_f32()+ submit_elapsed.as_secs_f32()),
                );

                info!("{} {}", "OK".bold().green(), tx);
            }
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
        let clock = get_clock(&self.rpc_client).await.expect("获取时钟失败");
        config
			.last_reset_at
			.saturating_add(EPOCH_DURATION)
			.saturating_sub(5) // Buffer
			.le(&clock.unix_timestamp)
    }

    async fn get_cutoff(&self, proof: Proof, buffer_time: u64) -> u64 {
        let clock = get_clock(&self.rpc_client).await.expect("获取时钟失败");
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
