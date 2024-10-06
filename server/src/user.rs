use std::{collections::HashMap, ops::Range};

use actix::{Actor, AsyncContext, Context, Handler};
use drillx::Hash;
use shared::{interaction::SolutionResponse, types::MinerKey};
use solana_program::{pubkey, pubkey::Pubkey};
use tracing::{debug, info, log::trace, warn};

pub use messages as UserMessage;

const STOP_SEND_OFFSET: u64 = 2;
const ONE_ROUND_MINING_TIME: u64 = 10;
const MAX_WORK_COUNT: u64 = 50000;
const MAX_WORK_BLOCK: u64 = 200_000;
const MINI_DIFFICULTY: u32 = 8;
const OMG_WALLET: Pubkey = pubkey!("omgVKDH5GfC4mM2YAgGot5FpB1vGGvXFhpk5jd9WKB5");

pub struct Epoch {
    inactive: bool,
    challenge: [u8; 32],
    difficulty: u32,
    hash: Hash,
    nonce: u64,
    works: Vec<Range<u64>>,
    cutoff: u64,
    deadline: i64,
}

pub struct UserActor {
    miners: HashMap<MinerKey, Epoch>,
}

impl UserActor {
    pub(crate) fn new(keys: Vec<MinerKey>) -> Self {
        // TODO 检查key重复
        let mut miners = HashMap::new();
        for key in keys {
            miners.entry(key).or_insert(Epoch {
                inactive: true,
                challenge: [0_u8; 32],
                difficulty: 0,
                hash: Default::default(),
                nonce: 0,
                works: vec![],
                cutoff: 0,
                deadline: 0,
            });
        }
        Self { miners }
    }

    fn generate_works(&self, inactive: bool) -> Vec<Range<u64>> {
        let count = if inactive {
            5
        } else {
            MAX_WORK_COUNT
        };
        let mut ranges = vec![];
        for i in 0..count {
            ranges.push(i * MAX_WORK_BLOCK..(i + 1) * MAX_WORK_BLOCK);
        }
        ranges
    }
}

impl Actor for UserActor {
    type Context = Context<Self>;

    fn started(&mut self, _: &mut Self::Context) {}
}

impl Handler<UserMessage::NextEpoch> for UserActor {
    type Result = ();

    fn handle(&mut self, msg: UserMessage::NextEpoch, _: &mut Self::Context) -> Self::Result {
        trace!("user: next epoch");

        let UserMessage::NextEpoch { miner, challenge, cutoff } = msg;

        if let Some(val) = self.miners.get_mut(&miner) {
            if val.challenge.ne(&challenge) {
                info!("new challenge: {}", bs58::encode(challenge).into_string());
                let inactive = cutoff == 0;
                let epoch = Epoch {
                    inactive,
                    challenge,
                    difficulty: 0,
                    hash: Default::default(),
                    cutoff,
                    works: self.generate_works(inactive),
                    deadline: shared::timestamp() + (cutoff as i64) * 1000,
                    nonce: 0,
                };
                self.miners.insert(miner, epoch);
            } else {
                info!("challenge exist: {}", bs58::encode(challenge).into_string());
            }
        }
    }
}

impl Handler<UserMessage::Peek> for UserActor {
    type Result = Vec<i32>;

    fn handle(&mut self, msg: UserMessage::Peek, _: &mut Self::Context) -> Self::Result {
        trace!("user: peek");
        msg.miners
            .iter()
            .map(|key| {
                match self.miners.get(key) {
                    None => -1,
                    Some(e) => e.difficulty as i32,
                }
            })
            .collect()
    }
}

impl Handler<UserMessage::FetchWork> for UserActor {
    type Result = Option<UserMessage::WorkResponse>;

    fn handle(&mut self, _: UserMessage::FetchWork, _: &mut Self::Context) -> Self::Result {
        trace!("user: fetch work");

        let now = shared::timestamp();

        let mut items: Vec<_> = self
            .miners
            .iter_mut()
            .filter(|(_, epoch)| {
                if epoch.works.len() == 0 {
                    return false;
                }

                if !epoch.inactive && now > epoch.deadline {
                    return false;
                }

                if epoch.inactive && epoch.difficulty > MINI_DIFFICULTY {
                    return false;
                }

                if !epoch.inactive
                    && epoch.deadline.saturating_sub(shared::timestamp())
                        < ((ONE_ROUND_MINING_TIME + STOP_SEND_OFFSET) * 1000) as i64
                {
                    return false;
                }

                return true;
            })
            .collect();

        debug!("miner count: {}", items.len());

        if items.len() > 0 {
            // 难度递增排序
            items.sort_by(|(_, a), (_, b)| a.difficulty.cmp(&b.difficulty));

            // 已经对工作数量做了过滤，所以直接解包
            let (key, epoch) = items.pop()?;

            Some(UserMessage::WorkResponse {
                miner: key.clone(),
                challenge: epoch.challenge,
                difficulty: epoch.difficulty,
                range: epoch.works.remove(0),
                deadline: epoch.deadline,
                work_time: ONE_ROUND_MINING_TIME,
            })
        } else {
            None
        }
    }
}

impl Handler<UserMessage::MiningResult> for UserActor {
    type Result = ();

    fn handle(&mut self, msg: UserMessage::MiningResult, _: &mut Self::Context) -> Self::Result {
        trace!("user: mining result");

        let UserMessage::MiningResult { miner, data } = msg;
        if let Some(epoch) = self.miners.get_mut(&miner) {
            if epoch.challenge.ne(&data.challenge) {
                warn!("challenge not equal");
                return;
            }
            if data.difficulty.gt(&(epoch.difficulty)) {
                info!("new difficulty: {} -> {}", epoch.difficulty, data.difficulty);
                epoch.difficulty = data.difficulty;
                epoch.nonce = data.nonce;
                epoch.hash = Hash { d: data.digest, h: data.hash }
            }
        }
    }
}

impl Handler<UserMessage::Solution> for UserActor {
    type Result = Option<SolutionResponse>;

    fn handle(&mut self, msg: UserMessage::Solution, ctx: &mut Self::Context) -> Self::Result {
        trace!("user: solution");
        if let Some(epoch) = self.miners.get_mut(&msg.miner) {
            if epoch.challenge.eq(&msg.challenge) {
                return Some(SolutionResponse {
                    nonce: epoch.nonce,
                    digest: epoch.hash.d,
                    hash: epoch.hash.h,
                    difficulty: epoch.difficulty,
                    omg_tip: 0,
                    omg_wallet: OMG_WALLET,
                });
            }
        }
        None
    }
}

impl Handler<UserMessage::CheckMinerKey> for UserActor {
    type Result = bool;

    fn handle(&mut self, msg: UserMessage::CheckMinerKey, _: &mut Self::Context) -> Self::Result {
        let keys = msg.miners;
        if keys.len() != self.miners.len() {
            return false;
        }
        for key in keys {
            if !self.miners.contains_key(&key) {
                return false;
            }
        }
        true
    }
}

pub mod messages {
    use std::ops::Range;

    use actix::Message;
    use shared::{interaction::SolutionResponse, types::MinerKey};

    pub struct WorkResponse {
        pub miner: MinerKey,
        pub challenge: [u8; 32],
        pub difficulty: u32,
        pub range: Range<u64>,
        pub deadline: i64,
        pub work_time: u64,
    }

    #[derive(Message)]
    #[rtype(result = "()")]
    pub(crate) struct NextEpoch {
        pub miner: MinerKey,
        pub challenge: [u8; 32],
        pub cutoff: u64,
    }

    #[derive(Message)]
    #[rtype(result = "Vec<i32>")]
    pub(crate) struct Peek {
        pub miners: Vec<MinerKey>,
    }

    #[derive(Message)]
    #[rtype(result = "Option<WorkResponse>")]
    pub struct FetchWork;

    #[derive(Message)]
    #[rtype(result = "()")]
    pub(crate) struct MiningResult {
        pub miner: MinerKey,
        pub data: shared::interaction::MiningResult,
    }

    #[derive(Message)]
    #[rtype(result = "Option<SolutionResponse>")]
    pub struct Solution {
        pub miner: MinerKey,
        pub challenge: [u8; 32],
    }

    #[derive(Message)]
    #[rtype(result = "bool")]
    pub struct CheckMinerKey {
        pub miners: Vec<MinerKey>,
    }
}
