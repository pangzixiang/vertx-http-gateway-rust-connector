use std::collections::HashMap;

pub struct RequestMessageInfoChunkBody {
    pub http_version: String,
    pub http_method: String,
    pub uri: String,
    pub headers: HashMap<String, String>
}

impl RequestMessageInfoChunkBody {
    pub fn new(request_message_info_chunk_body: String) -> RequestMessageInfoChunkBody {
        let lines: Vec<&str> = request_message_info_chunk_body.split("\r\n").collect();
        let first_line: Vec<&str> = lines[0].split(" ").collect();
        let http_version = String::from(first_line[0]);
        let http_method = String::from(first_line[1]);
        let uri = String::from(first_line[2]).trim().to_string();
        let mut headers = HashMap::new();
        for i in 1..lines.len() {
            if let Some(line) = lines[i].trim().split_once(":"){
                let key = line.0.trim();
                let value = line.1.trim();
                if key.len() > 0 && value.len() > 0 {
                    headers.insert(key.to_lowercase(), String::from(value));
                }
            }
        }
        RequestMessageInfoChunkBody {
            http_version,
            http_method,
            uri,
            headers
        }
    }
}