use std::{fs::File, io};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub user: String,
    pub server_host: String,
    pub rpc: Option<String>,
    // pub jito_url: Option<String>,
    // pub dynamic_fee_url: Option<String>,
    pub fee_payer: Option<String>,
}

pub fn load_config_file(config_file: String) -> Result<AppConfig, io::Error> {
    let file = File::open(config_file.clone())
        .expect(format!("{config_file} config file not found").as_str());
    let config = serde_json::from_reader(file)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{err:?}")))?;
    Ok(config)
}
