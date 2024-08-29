use reqwest::{Method, Url};
use serde::{de, ser};

use shared::interaction::{
    BlockHash,
    BlockHashResponse,
    Challenge,
    LoginResponse,
    NextEpoch,
    RestfulResponse,
    User,
};

pub struct ServerAPI {
    pub name: String,
    pub url: String,
}

impl ServerAPI {
    /// login and get rpc url
    pub async fn login(&self, keys: Vec<String>) -> anyhow::Result<(String, String)> {
        let user = User {
            name: self.name.clone(),
            keys,
        };
        let resp = self.request::<_, LoginResponse>("/api/v1/login", Method::POST, user).await?;
        if resp.code == 200 {
            let data = resp.data.unwrap();
            return Ok((data.rpc, data.jito_url));
        }
        anyhow::bail!(resp.message.unwrap());
    }

    /// send then next epoch's challenge adn cutoff
    pub async fn next_epoch(
        &self,
        key: String,
        challenge: Challenge,
        cutoff: u64,
    ) -> anyhow::Result<()> {
        let epoch = NextEpoch {
            user: self.name.clone(),
            key,
            challenge,
            cutoff,
        };
        let resp = self.request::<_, ()>("/api/v1/epoch", Method::POST, epoch).await?;
        if resp.code == 200 {
            return Ok(());
        }
        anyhow::bail!(resp.message.unwrap());
    }

    /// get transaction by block hash
    pub async fn block_hash(
        &self,
        key: String,
        data: [u8; 32],
    ) -> anyhow::Result<BlockHashResponse> {
        let block = BlockHash {
            user: self.name.clone(),
            key,
            data,
        };

        let resp = self
            .request::<_, BlockHashResponse>("/api/v1/transaction", Method::POST, block)
            .await?;

        if resp.code == 200 {
            let data = resp.data.unwrap();
            return Ok(data);
        }

        anyhow::bail!(resp.message.unwrap());
    }

    pub async fn peek_difficulty(&self, keys: Vec<String>) -> anyhow::Result<Vec<u32>> {
        let resp = self.request::<_, Vec<u32>>("/api/v1/difficulty", Method::POST, keys).await?;

        if resp.code == 200 {
            let data = resp.data.unwrap();
            return Ok(data);
        }

        anyhow::bail!(resp.message.unwrap());
    }

    /// base request
    async fn request<T, R>(
        &self,
        endpoint: &str,
        method: Method,
        data: T,
    ) -> anyhow::Result<RestfulResponse<R>>
    where
        T: ser::Serialize,
        R: de::DeserializeOwned, {
        let mut url = Url::parse(&self.url)?;
        url = url.join(&endpoint)?;

        let client = reqwest::Client::new();

        let response = match method {
            Method::GET => client.get(url).header("Content-Type", "html/text").send().await,
            Method::POST => {
                client.post(url).header("Content-Type", "application/json").json(&data).send().await
            }
            _ => anyhow::bail!("unsupported method: {method}"),
        };

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

        let response: RestfulResponse<R> = match serde_json::from_str(&text) {
            Ok(response) => response,
            Err(err) => {
                anyhow::bail!(
                    "fail to deserialize response: {err:#}, response: {text}, status: {status}"
                )
            }
        };

        Ok(response)
    }
}
