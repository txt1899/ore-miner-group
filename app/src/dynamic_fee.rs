use crate::Miner;
use ore_api::consts::BUS_ADDRESSES;
use reqwest::Client;
use serde_json::{json, Value};
use solana_client::rpc_response::RpcPrioritizationFee;
use url::Url;

enum FeeStrategy {
    Helius,
    Triton,
    Alchemy,
    Quiknode,
}

impl Miner {
    pub async fn dynamic_fee(&self) -> Result<u64, String> {
        // Get url
        let rpc_url = self.dynamic_fee_url.clone().unwrap_or(self.rpc_client.url());

        // Select fee estiamte strategy
        let host = Url::parse(&rpc_url).unwrap().host_str().unwrap().to_string();
        let strategy = if host.contains("helius-rpc.com") {
            FeeStrategy::Helius
        } else if host.contains("alchemy.com") {
            FeeStrategy::Alchemy
        } else if host.contains("quiknode.pro") {
            FeeStrategy::Quiknode
        } else if host.contains("rpcpool.com") {
            FeeStrategy::Triton
        } else {
            return Err("Dynamic fees not supported by this RPC.".to_string());
        };

        // Build fee estimate request
        let client = Client::new();
        let ore_addresses: Vec<String> = std::iter::once(ore_api::ID.to_string())
            .chain(BUS_ADDRESSES.iter().map(|pubkey| pubkey.to_string()))
            .collect();
        let body = match strategy {
            FeeStrategy::Helius => {
                json!({
                    "jsonrpc": "2.0",
                    "id": "priority-fee-estimate",
                    "method": "getPriorityFeeEstimate",
                    "params": [{
                        "accountKeys": ore_addresses,
                        "options": {
                            "recommended": true
                        }
                    }]
                })
            }
            FeeStrategy::Alchemy => {
                json!({
                    "jsonrpc": "2.0",
                    "id": "priority-fee-estimate",
                    "method": "getRecentPrioritizationFees",
                    "params": [
                        ore_addresses
                    ]
                })
            }
            FeeStrategy::Quiknode => {
                json!({
                    "jsonrpc": "2.0",
                    "id": "1",
                    "method": "qn_estimatePriorityFees",
                    "params": {
                        "account": BUS_ADDRESSES[0].to_string(),
                        "last_n_blocks": 100
                    }
                })
            }
            FeeStrategy::Triton => {
                json!({
                    "jsonrpc": "2.0",
                    "id": "priority-fee-estimate",
                    "method": "getRecentPrioritizationFees",
                    "params": [
                        ore_addresses,
                        {
                            "percentile": 5000,
                        }
                    ]
                })
            }
        };

        // Send request
        let response: Value =
            client.post(rpc_url).json(&body).send().await.unwrap().json().await.unwrap();

        // Parse response
        let calculated_fee = match strategy {
            FeeStrategy::Helius => {
                response["result"]["priorityFeeEstimate"]
                    .as_f64()
                    .map(|fee| fee as u64)
                    .ok_or_else(|| format!("Failed to parse priority fee response: {:?}", response))
            }
            FeeStrategy::Quiknode => response["result"]["per_compute_unit"]["medium"]
                .as_f64()
                .map(|fee| fee as u64)
                .ok_or_else(|| {
                    "Please enable the Solana Priority Fee API add-on in your QuickNode account."
                        .to_string()
                }),
            FeeStrategy::Alchemy => {
                response["result"]
                    .as_array()
                    .and_then(|arr| {
                        Some(
                            arr.into_iter()
                                .map(|v| v["prioritizationFee"].as_u64().unwrap())
                                .collect::<Vec<u64>>(),
                        )
                    })
                    .and_then(|fees| {
                        Some(
                            ((fees.iter().sum::<u64>() as f32 / fees.len() as f32).ceil() * 1.2)
                                as u64,
                        )
                    })
                    .ok_or_else(|| format!("Failed to parse priority fee response: {:?}", response))
            }
            FeeStrategy::Triton => {
                serde_json::from_value::<Vec<RpcPrioritizationFee>>(response["result"].clone())
                    .map(|prioritization_fees| {
                        estimate_prioritization_fee_micro_lamports(prioritization_fees)
                    })
                    .map_err(|error| {
                        format!(
                            "Failed to parse priority fee response: {response:?}, error: {error}"
                        )
                    })
            }
        };

        // Check if the calculated fee is higher than max
        match calculated_fee {
            Err(err) => Err(err),
            Ok(fee) => {
                if let Some(max_fee) = self.priority_fee {
                    Ok(fee.min(max_fee))
                } else {
                    Ok(fee)
                }
            }
        }
    }
}

/// Our estimate is the average over the last 20 slots
pub fn estimate_prioritization_fee_micro_lamports(
    prioritization_fees: Vec<RpcPrioritizationFee>,
) -> u64 {
    let prioritization_fees = prioritization_fees
        .into_iter()
        .rev()
        .take(20)
        .map(
            |RpcPrioritizationFee {
                 prioritization_fee,
                 ..
             }| prioritization_fee,
        )
        .collect::<Vec<_>>();
    if prioritization_fees.is_empty() {
        panic!("Response does not contain any prioritization fees");
    }

    let prioritization_fee =
        prioritization_fees.iter().sum::<u64>() / prioritization_fees.len() as u64;

    prioritization_fee
}
