use reqwest::{Method, Url};
use serde::{de, ser};
use shared::{
    interaction::{NextEpoch, Peek, RestfulResponse, Solution, SolutionResponse, User},
    types::{MinerKey, UserName},
};

pub struct ServerAPI {
    pub url: String,
}

impl ServerAPI {
    /// login send miner pubkey to server
    pub async fn login(&self, user: UserName, miners: Vec<MinerKey>) -> anyhow::Result<()> {
        let payload = User {
            user,
            miners,
        };
        let resp = self.request::<_, ()>("/api/v1/login", Method::POST, payload).await?;
        if resp.code == 200 {
            return Ok(());
        }
        anyhow::bail!(resp.message.unwrap());
    }

    /// send then next epoch's challenge and cutoff
    pub async fn next_epoch(
        &self,
        user: UserName,
        miner: MinerKey,
        challenge: [u8; 32],
        cutoff: u64,
    ) -> anyhow::Result<()> {
        let payload = NextEpoch {
            user,
            miner,
            challenge,
            cutoff,
        };
        let resp = self.request::<_, ()>("/api/v1/epoch", Method::POST, payload).await?;
        if resp.code == 200 {
            return Ok(());
        }
        anyhow::bail!(resp.message.unwrap());
    }

    // check difficulty when miners are inactive
    pub async fn peek_difficulty(&self, data: Peek) -> anyhow::Result<Vec<u32>> {
        let resp = self.request::<_, Vec<u32>>("/api/v1/difficulty", Method::POST, data).await?;
        if resp.code == 200 {
            let data = resp.data.unwrap();
            return Ok(data);
        }
        anyhow::bail!(resp.message.unwrap());
    }

    // get the best solution
    pub async fn get_solution(
        &self,
        user: UserName,
        miner: MinerKey,
        challenge: [u8; 32],
    ) -> anyhow::Result<SolutionResponse> {
        let payload = Solution {
            user,
            miner,
            challenge,
        };

        let resp =
            self.request::<_, SolutionResponse>("/api/v1/solution", Method::POST, payload).await?;

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
        url = url.join(endpoint)?;

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
