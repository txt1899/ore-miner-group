use std::{fs::File, io, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub name: String,
    pub server_host: String,
    pub rpc: Option<String>,
    pub jito_url: Option<String>,
}

pub fn load_config_file<P>(config_file: P) -> Result<AppConfig, io::Error>
where
    P: AsRef<Path>, {
    let file = File::open(config_file).expect("config file not found");
    let config = serde_json::from_reader(file)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{err:?}")))?;
    Ok(config)
}
