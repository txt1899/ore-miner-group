pub mod interaction;
pub mod jito;
pub mod types;
pub mod utils;

pub fn timestamp() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
