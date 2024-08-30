pub mod interaction;
pub mod jito;
pub mod utils;
pub mod types;

pub fn timestamp() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
