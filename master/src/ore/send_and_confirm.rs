use std::{str::FromStr, time::Duration};

use chrono::Local;
use colored::*;
use ore_api::error::OreError;
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::{
    instruction::Instruction,
    native_token::{lamports_to_sol, sol_to_lamports},
    pubkey::Pubkey,
    system_instruction::transfer,
};
use solana_rpc_client::spinner;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};
use tracing::{debug, error, info, warn};

use crate::ore::{utils, Miner};

const MIN_SOL_BALANCE: f64 = 0.005;

const RPC_RETRIES: usize = 0;
const _SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 3;
const CONFIRM_RETRIES: usize = 8;

const CONFIRM_DELAY: u64 = 500;
const GATEWAY_DELAY: u64 = 0; //300;

pub enum ComputeBudget {
    Dynamic,
    Fixed(u32),
}

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        compute_budget: ComputeBudget,
        skip_confirm: bool,
        tip: Option<(String, u64)>,
    ) -> ClientResult<Signature> {
        let signer = self.signer();
        let fee_payer = self.fee_payer();

        let client = self.rpc_client.clone();
        let mut send_client = self.rpc_client.clone();

        // Return error, if balance is zero
        self.check_balance().await;

        // Set compute budget
        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                // TODO simulate
                // final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(1_400_000))
            }
            ComputeBudget::Fixed(cus) => {
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus))
            }
        }

        // Set compute unit price
        final_ixs
            .push(ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee.unwrap_or(0)));

        // Add in user instructions
        final_ixs.extend_from_slice(ixs);

        if let Some((account, amount)) = tip {
            send_client = self.jito_client.clone();
            final_ixs.push(transfer(
                &signer.pubkey(),
                &Pubkey::from_str(account.as_str()).unwrap(),
                amount,
            ));
            info!("Jito小费: {} SOL", lamports_to_sol(amount));
        }

        // Build tx
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };

        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));

        // Submit tx
        let mut attempts = 0;
        info!("开始提交");
        let mut sigs = vec![];
        loop {
            // Sign tx with a new blockhash (after approximately ~45 sec)
            if attempts % 10 == 0 {
                // Reset the compute unit price
                if self.dynamic_fee {
                    let fee = match self.dynamic_fee().await {
                        Ok(fee) => {
                            info!("优先费: {}", fee);
                            fee
                        }
                        Err(err) => {
                            let fee = self.priority_fee.unwrap_or(0);
                            info!(
                                "  {} {} 使用固定优先费: {}",
                                "WARNING".bold().yellow(),
                                err,
                                fee
                            );
                            fee
                        }
                    };

                    final_ixs.remove(1);
                    final_ixs.insert(1, ComputeBudgetInstruction::set_compute_unit_price(fee));
                    tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));
                }

                // Resign the tx
                let (hash, _slot) = utils::get_latest_blockhash_with_retries(&client).await?;
                if signer.pubkey() == fee_payer.pubkey() {
                    tx.sign(&[&signer], hash);
                } else {
                    tx.sign(&[&signer, &fee_payer], hash);
                }
            }

            // Send transaction
            match send_client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    // Skip confirmation
                    if skip_confirm {
                        info!("Sent: {}", sig);
                        return Ok(sig);
                    }
                    sigs.push(sig);
                    debug!("signature: {:?}", sig);

                    // Confirm transaction
                    'confirm: for _ in 0..CONFIRM_RETRIES {
                        tokio::time::sleep(Duration::from_millis(CONFIRM_DELAY)).await;
                        match client.get_signature_statuses(&sigs[..]).await {
                            Ok(signature_statuses) => {
                                for (index, status) in signature_statuses.value.iter().enumerate() {
                                    if let Some(status) = status {
                                        if let Some(ref err) = status.err {
                                            match err {
                                                // Instruction error
                                                solana_sdk::transaction::TransactionError::InstructionError(_, err) => {
                                                    match err {
                                                        // Custom instruction error, parse into OreError
                                                        solana_program::instruction::InstructionError::Custom(err_code) => {
                                                            match err_code {
                                                                e if (OreError::NeedsReset as u32).eq(e) => {
                                                                    // attempts = 0;
                                                                    // 这是一个合约错误，hash随机种子并没更新，可以重新发送行的交易
                                                                    // 是否可以清空sigs，由于链上确认的延迟，不能保证其中某个tx无效
                                                                    // 所有仅删除引发错误的tx，
                                                                    sigs.remove(index);
                                                                    error!( "Needs reset. 重试...");
                                                                    break 'confirm
                                                                },
                                                                _ => {
                                                                    error!("{err:?}");
                                                                    return Err(ClientError {
                                                                        request: None,
                                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                                    });
                                                                }
                                                            }
                                                        },

                                                        // Non custom instruction error, return
                                                        _ => {
                                                            error!("{err:?}");
                                                            return Err(ClientError {
                                                                request: None,
                                                                kind: ClientErrorKind::Custom(err.to_string()),
                                                            });
                                                        }
                                                    }
                                                },

                                                // Non instruction error, return
                                                _ => {
                                                    error!("{err:?}");
                                                    return Err(ClientError {
                                                        request: None,
                                                        kind: ClientErrorKind::Custom(err.to_string()),
                                                    });
                                                }
                                            }
                                        } else if let Some(ref confirmation) =
                                            status.confirmation_status
                                        {
                                            debug!("confirmation: {:?}", confirmation);
                                            match confirmation {
                                                TransactionConfirmationStatus::Processed => {}
                                                TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    return Ok(sig);
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
                }

                // Handle submit errors
                Err(err) => {
                    error!("{:?}", err.kind());
                }
            }

            // Retry
            tokio::time::sleep(Duration::from_millis(GATEWAY_DELAY)).await;

            attempts += 1;
            if attempts >= GATEWAY_RETRIES {
                warn!("最大尝试，放弃提交");
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max retries".into()),
                });
            }
        }
    }

    pub async fn check_balance(&self) {
        // Throw error if balance is less than min
        if let Ok(balance) = self.rpc_client.get_balance(&self.fee_payer().pubkey()).await {
            if balance <= sol_to_lamports(MIN_SOL_BALANCE) {
                panic!(
                    "{} 余额不足: {} SOL\n请充值不少于 {} SOL",
                    "ERROR".bold().red(),
                    lamports_to_sol(balance),
                    MIN_SOL_BALANCE
                );
            }
        }
    }

    // TODO
    fn _simulate(&self) {

        // Simulate tx
        // let mut sim_attempts = 0;
        // 'simulate: loop {
        //     let sim_res = client
        //         .simulate_transaction_with_config(
        //             &tx,
        //             RpcSimulateTransactionConfig {
        //                 sig_verify: false,
        //                 replace_recent_blockhash: true,
        //                 commitment: Some(self.rpc_client.commitment()),
        //                 encoding: Some(UiTransactionEncoding::Base64),
        //                 accounts: None,
        //                 min_context_slot: Some(slot),
        //                 inner_instructions: false,
        //             },
        //         )
        //         .await;
        //     match sim_res {
        //         Ok(sim_res) => {
        //             if let Some(err) = sim_res.value.err {
        //                 println!("Simulaton error: {:?}", err);
        //                 sim_attempts += 1;
        //             } else if let Some(units_consumed) = sim_res.value.units_consumed {
        //                 if dynamic_cus {
        //                     println!("Dynamic CUs: {:?}", units_consumed);
        //                     let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(
        //                         units_consumed as u32 + 1000,
        //                     );
        //                     let cu_price_ix =
        //                         ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
        //                     let mut final_ixs = vec![];
        //                     final_ixs.extend_from_slice(&[cu_budget_ix, cu_price_ix]);
        //                     final_ixs.extend_from_slice(ixs);
        //                     tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));
        //                 }
        //                 break 'simulate;
        //             }
        //         }
        //         Err(err) => {
        //             println!("Simulaton error: {:?}", err);
        //             sim_attempts += 1;
        //         }
        //     }

        //     // Abort if sim fails
        //     if sim_attempts.gt(&SIMULATION_RETRIES) {
        //         return Err(ClientError {
        //             request: None,
        //             kind: ClientErrorKind::Custom("Simulation failed".into()),
        //         });
        //     }
        // }
    }
}
