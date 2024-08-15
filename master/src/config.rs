use std::{fs::File, io, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MinerConfig {
    pub rpc: String,
    pub keypair_path: String,
    pub fee_payer: Option<String>,
    pub buffer_time: u64,
    pub dynamic_fee_url: Option<String>,
    pub port: u16,
}

pub fn load_config_file<P>(config_file: P) -> Result<MinerConfig, io::Error>
where
    P: AsRef<Path>, {
    let file = File::open(config_file).expect("配置文件不存在");
    let config = serde_json::from_reader(file)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{err:?}")))?;
    Ok(config)
}
