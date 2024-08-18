use crate::{
    ore::{send_and_confirm::ComputeBudget, utils::proof_pubkey, Miner},
    websocket::{jito::JitoActor, messages},
};
use actix::Addr;
use solana_sdk::signature::Signer;
use tracing::info;

impl Miner {
    pub async fn open(&self, jito: Addr<JitoActor>) {
        // Return early if miner is already registered
        let signer = self.signer();
        let fee_payer = self.fee_payer();
        let proof_address = proof_pubkey(signer.pubkey());
        if self.rpc_client.get_account(&proof_address).await.is_ok() {
            return;
        }

        info!("注册矿工...");
        let value = jito.send(messages::WithTip).await.expect("获取jito小费失败");
        let ix = ore_api::instruction::open(signer.pubkey(), signer.pubkey(), fee_payer.pubkey());
        self.send_and_confirm(&[ix], ComputeBudget::Fixed(400_000), false, value, None).await.ok();
    }
}
