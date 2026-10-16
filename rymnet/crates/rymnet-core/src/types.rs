use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedEnvelope<T> {
    pub payload: T,
    pub signature: String,
    pub signed_at: DateTime<Utc>,
}
