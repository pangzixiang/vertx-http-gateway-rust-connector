use bytes::{Buf, Bytes};
use tokio_tungstenite::{tungstenite::protocol::Message};

#[derive(Debug)]
pub struct MessageChunk {
    pub chunk_type: i8,
    pub request_id: u64,
    pub chunk_body: Bytes
}

impl MessageChunk {
    pub fn new(message: Message) -> MessageChunk {
        let data = message.into_data();
        let chunk_type = Bytes::from(data.clone()).slice(0..1).get_i8();
        let request_id = Bytes::from(data.clone()).slice(1..9).get_u64();
        MessageChunk {
            chunk_type,
            request_id,
            chunk_body: Bytes::from(data.clone()).slice(9..)
        }
    }
}
