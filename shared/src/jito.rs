use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

use futures_util::{stream::StreamExt, SinkExt};
use lazy_static::lazy_static;
use rand::Rng;
use serde::{de, Deserialize};
use serde_json::{json, Value};
use solana_program::{native_token::lamports_to_sol, pubkey};
use solana_sdk::{pubkey::Pubkey, signature::Signature, transaction::Transaction};
use solana_transaction_status::{Encodable, EncodedTransaction, UiTransactionEncoding};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
    time,
};
use tokio_tungstenite::tungstenite::{Error, Message};
use tracing::*;

lazy_static! {
    static ref JITO_TIPS: RwLock<JitoTips> = RwLock::new(JitoTips::default());
}

pub const FEE_PER_SIGNER: u64 = 5000;

pub const SLOT_EXPIRATION: u64 = 151 + 5;

pub const FETCH_ACCOUNT_LIMIT: usize = 100;

pub const TRANSFER_BATCH_SIZE: usize = 21;

pub const JITO_RECIPIENTS: [Pubkey; 8] = [
    pubkey!("96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5"),
    pubkey!("HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe"),
    pubkey!("Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY"),
    pubkey!("ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49"),
    pubkey!("DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh"),
    pubkey!("ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt"),
    pubkey!("DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL"),
    pubkey!("3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT"),
];

pub fn pick_jito_recipient() -> &'static Pubkey {
    &JITO_RECIPIENTS[rand::thread_rng().gen_range(0..JITO_RECIPIENTS.len())]
}

#[derive(Debug, Deserialize)]
pub struct JitoResponse<T> {
    pub result: T,
}

async fn make_jito_request<T>(method: &'static str, params: Value) -> anyhow::Result<T>
where
    T: de::DeserializeOwned, {
    let response = reqwest::Client::new()
        .post("https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles")
        .header("Content-Type", "application/json")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(err) => anyhow::bail!("fail to send request: {err}"),
    };

    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(err) => anyhow::bail!("fail to read response content: {err:#}"),
    };

    if !status.is_success() {
        anyhow::bail!("status code: {status}, response: {text}");
    }

    let response: T = match serde_json::from_str(&text) {
        Ok(response) => response,
        Err(err) => {
            anyhow::bail!(
                "fail to deserialize response: {err:#}, response: {text}, status: {status}"
            )
        }
    };

    Ok(response)
}

pub async fn send_bundle(bundle: Vec<Transaction>) -> anyhow::Result<(Signature, String)> {
    let signature =
        *bundle.first().expect("empty bundle").signatures.first().expect("empty transaction");

    let bundle = bundle
        .into_iter()
        .map(|tx| {
            match tx.encode(UiTransactionEncoding::Binary) {
                EncodedTransaction::LegacyBinary(b) => b,
                _ => panic!("impossible"),
            }
        })
        .collect::<Vec<_>>();

    let response: JitoResponse<String> = make_jito_request("sendBundle", json!([bundle])).await?;

    Ok((signature, response.result))
}

pub fn build_bribe_ix(pubkey: &Pubkey, value: u64) -> solana_sdk::instruction::Instruction {
    solana_sdk::system_instruction::transfer(pubkey, pick_jito_recipient(), value)
}

#[derive(Debug, Deserialize)]
pub struct Tip {
    pub time: String,
    pub landed_tips_25th_percentile: f64,
    pub landed_tips_50th_percentile: f64,
    pub landed_tips_75th_percentile: f64,
    pub landed_tips_95th_percentile: f64,
    pub landed_tips_99th_percentile: f64,
    pub ema_landed_tips_50th_percentile: f64,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct JitoTips {
    #[serde(rename = "landed_tips_25th_percentile")]
    pub p25_landed: f64,

    #[serde(rename = "landed_tips_50th_percentile")]
    pub p50_landed: f64,

    #[serde(rename = "landed_tips_75th_percentile")]
    pub p75_landed: f64,

    #[serde(rename = "landed_tips_95th_percentile")]
    pub p95_landed: f64,

    #[serde(rename = "landed_tips_99th_percentile")]
    pub p99_landed: f64,

    #[serde(rename = "ema_landed_tips_50th_percentile")]
    pub ema_p50_landed: f64,
}

impl JitoTips {
    pub fn p50(&self) -> u64 {
        (self.p50_landed * 1e9f64) as u64
    }

    pub fn p25(&self) -> u64 {
        (self.p25_landed * 1e9f64) as u64
    }

    pub fn ema50(&self) -> u64 {
        (self.ema_p50_landed * 1e9f64) as u64
    }
}

impl Display for JitoTips {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tips(p25 = {}, p50 = {}, p75 = {}, p95 = {}, p99 = {}, ema-p50 = {}) SOL",
            lamports_to_sol((self.p25_landed * 1e9f64) as u64),
            lamports_to_sol((self.p50_landed * 1e9f64) as u64),
            lamports_to_sol((self.p75_landed * 1e9f64) as u64),
            lamports_to_sol((self.p95_landed * 1e9f64) as u64),
            lamports_to_sol((self.p99_landed * 1e9f64) as u64),
            lamports_to_sol((self.ema_p50_landed * 1e9f64) as u64)
        )
    }
}

pub async fn subscribe_jito_tips() -> JoinHandle<()> {
    tokio::spawn({
        async move {
            let url = "ws://bundles-api-rest.jito.wtf/api/v1/bundles/tip_stream";

            loop {
                let stream = match tokio_tungstenite::connect_async(url).await {
                    Ok((ws_stream, _)) => ws_stream,
                    Err(err) => {
                        error!("fail to connect to jito tip stream: {err:#}");
                        tokio::time::sleep(time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let (write, read) = stream.split();

                let write = Arc::new(Mutex::new(write));

                read.for_each(|message| {
                    async {
                        let data = match message {
                            Ok(msg) => {
                                match msg {
                                    Message::Text(data) => {
                                        match serde_json::from_str::<Vec<JitoTips>>(&data) {
                                            Ok(t) => Some(t),
                                            Err(err) => {
                                                error!("fail to parse jito tips: {err:#}");
                                                return;
                                            }
                                        }
                                    }
                                    Message::Ping(data) => {
                                        let mut guard = write.lock().await;
                                        let _ = guard.send(Message::Ping(data)).await;
                                        None
                                    }
                                    _ => None,
                                }
                            }
                            Err(err) => {
                                error!("fail to read jito tips message: {err:#}");
                                return;
                            }
                        };

                        if let Some(data) = data {
                            if data.is_empty() {
                                return;
                            }
                            debug!("jito: {data:?}");
                            *JITO_TIPS.write().await = *data.first().unwrap();
                        }
                    }
                })
                .await;

                info!("jito tip stream disconnected, retries in 5 seconds");
                tokio::time::sleep(time::Duration::from_secs(5)).await;
            }
        }
    })
}

pub async fn get_jito_tips() -> JitoTips {
    let jito = JITO_TIPS.read().await;
    info!("{jito:?}");
    jito.clone()
}
