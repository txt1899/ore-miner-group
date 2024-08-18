use std::sync::Arc;

use crate::lua::LuaScript;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::{read_keypair_file, Keypair};

pub struct Miner {
    pub keypair_filepath: Option<String>,
    pub priority_fee: Option<u64>,
    pub dynamic_fee_url: Option<String>,
    pub dynamic_fee: bool,
    pub rpc_client: Arc<RpcClient>,
    pub fee_payer_filepath: Option<String>,
    pub jito_client: Arc<RpcClient>,
    pub buffer_time: u64,
    pub script: LuaScript,
}

impl Miner {
    pub fn new(
        rpc_client: Arc<RpcClient>,
        priority_fee: Option<u64>,
        keypair_filepath: Option<String>,
        dynamic_fee_url: Option<String>,
        dynamic_fee: bool,
        fee_payer_filepath: Option<String>,
        jito_client: Arc<RpcClient>,
        buffer_time: u64,
        script: LuaScript,
    ) -> Self {
        Self {
            rpc_client,
            keypair_filepath,
            priority_fee,
            dynamic_fee_url,
            dynamic_fee,
            fee_payer_filepath,
            jito_client,
            buffer_time,
            script
        }
    }

    pub fn signer(&self) -> Keypair {
        match self.keypair_filepath.clone() {
            Some(filepath) => {
                read_keypair_file(filepath.clone())
                    .expect(format!("No keypair found at {}", filepath).as_str())
            }
            None => panic!("No keypair provided"),
        }
    }

    pub fn fee_payer(&self) -> Keypair {
        match self.fee_payer_filepath.clone() {
            Some(filepath) => {
                read_keypair_file(filepath.clone())
                    .expect(format!("No fee payer keypair found at {}", filepath).as_str())
            }
            None => panic!("No fee payer keypair provided"),
        }
    }
}
