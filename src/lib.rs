use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use bytes::Bytes;
use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Body, Method, StatusCode, Version};
use tokio::sync::Mutex;

use tokio::time;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::message_chunk::MessageChunk;
use crate::request_message_info_chunk_body::RequestMessageInfoChunkBody;

mod message_chunk;
mod request_message_info_chunk_body;

const CHUNK_TYPE_VEC: [i8; 5] = [-2, 0, 1, 2, 3];

pub struct VertxHttpGatewayConnector {
    connection_url: String,
    service_host: String,
    service_port: u16,
}

pub struct ConnectorOptions {
    pub register_host: String,
    pub register_port: u16,
    pub service_host: String,
    pub service_name: String,
    pub service_port: u16,
    pub register_path: String,
}

impl VertxHttpGatewayConnector {
    pub fn new(options: ConnectorOptions) -> VertxHttpGatewayConnector {
        VertxHttpGatewayConnector {
            connection_url: format!("ws://{}:{}{}?serviceName={}&servicePort={}&instance={}",
                                    options.register_host,
                                    options.register_port,
                                    options.register_path,
                                    options.service_name,
                                    options.service_port,
                                    uuid::Uuid::new_v4().to_string()),
            service_host: options.service_host,
            service_port: options.service_port,
        }
    }

    pub async fn start(&self) {
        let (ws_stream, _) = connect_async(&self.connection_url).await.expect(&format!("Failed to connect to {}", &self.connection_url));
        println!("Connector succeeded to connect {}!", &self.connection_url);

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (ws_tx, mut ws_rx) = mpsc::unbounded::<Result<Vec<u8>, Box<dyn Error + Send + Sync>>>();

        let ws_future = tokio::spawn(async move {
            while let Some(message) = ws_rx.next().await {
                let b = message.unwrap();
                if b == "ping".as_bytes().to_vec() {
                    ws_write.send(Message::Ping(b)).await.expect("");
                } else {
                    ws_write.send(Message::Binary(b)).await.expect("TODO: panic message");
                }
            }
        });

        let ws_tx_ping = ws_tx.clone();
        let ping_future = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                println!("send ping to server");
                ws_tx_ping.unbounded_send(Ok("ping".as_bytes().to_vec())).unwrap();
            }
        });

        let service_host = self.service_host.clone();
        let service_port = self.service_port.clone();
        let request_in_progress = Arc::new(Mutex::new(HashMap::new()));
        
        let process_message_future = tokio::spawn(async move {
            let client = reqwest::Client::builder().no_proxy().build().expect("should be able to build reqwest client");
            let mut body_sender = None;
            while let Some(msg) = ws_read.next().await {
                let msg = msg.unwrap();
                let ws_tx_clone = ws_tx.clone();
                let request_in_progress_clone = Arc::clone(&request_in_progress);
                if msg.is_close() {}
                if msg.is_ping() {
                    println!("receive ping from server");
                }
                if msg.is_text() {
                    println!("receive text from server: {}", msg.clone().into_text().unwrap());
                }
                if msg.is_pong() {
                    println!("receive pong from server");
                }
                if msg.is_binary() {
                    let message_chunk = MessageChunk::new(msg.clone());
                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[1] {
                        let request_message_info_chunk_body = RequestMessageInfoChunkBody::new(String::from_utf8(message_chunk.chunk_body.to_vec()).unwrap());
                        println!("start to handle request for {} {}", request_message_info_chunk_body.http_method, request_message_info_chunk_body.uri);
                        
                        request_in_progress_clone.lock().await.insert(message_chunk.request_id, true);

                        let mut request_builder = client.request(
                            Method::from_bytes(request_message_info_chunk_body.http_method.as_bytes()).expect(&format!("unable to parse request method {}", request_message_info_chunk_body.http_method)),
                            format!("http://{}:{}{}", service_host, service_port, request_message_info_chunk_body.uri));

                        for (key, value) in request_message_info_chunk_body.headers {
                            request_builder = request_builder.header(key, value);
                        }

                        if request_message_info_chunk_body.http_method != "GET" {
                            let (sender, recv) = mpsc::unbounded::<Result<Bytes, Box<dyn Error + Send + Sync>>>();
                            body_sender = Some(sender);
                            request_builder = request_builder.body(Body::wrap_stream(recv));
                        }

                        tokio::spawn(async move {
                            let response = request_builder.send().await.expect("should get response");

                            println!("Response: {}", response.status());

                            let response_message_info_chunk_body = build_response_info_chunk_body(response.headers(), response.version(), response.status());
                            let mut info_chunk_bytes: Vec<u8> = CHUNK_TYPE_VEC[1].to_be_bytes().to_vec();
                            info_chunk_bytes.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                            info_chunk_bytes.append(&mut response_message_info_chunk_body.clone());
                            ws_tx_clone.unbounded_send(Ok(info_chunk_bytes)).expect("should send first info response chunk");

                            let mut stream = response.bytes_stream();

                            while let Some(b) = stream.next().await {
                                if let Some(is_in_progress) = request_in_progress_clone.lock().await.get(&message_chunk.request_id) {
                                    let mut body_buffer = CHUNK_TYPE_VEC[2].to_be_bytes().to_vec();
                                    body_buffer.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                                    body_buffer.append(&mut b.unwrap().to_vec());
                                    println!("Response body: {:?}", body_buffer);
                                    ws_tx_clone.unbounded_send(Ok(body_buffer)).expect("should send response body chunk");
                                } else { 
                                    break;
                                }
                            }

                            let mut end_buffer = CHUNK_TYPE_VEC[3].to_be_bytes().to_vec();
                            end_buffer.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                            ws_tx_clone.unbounded_send(Ok(end_buffer)).expect("should send response body end chunk");

                            println!("proxy done");
                        });
                    }

                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[2] {
                        println!("received request body chunk: {:?}", message_chunk.chunk_body);
                        if let Some(sender) = body_sender.clone() {
                            sender.unbounded_send(Ok(message_chunk.chunk_body)).expect("should send request body chunk");
                        }
                    }

                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[3] {
                        println!("received request end notification");
                        if let Some(sender) = body_sender.clone() {
                            println!("send body end notification");
                            sender.close_channel();
                        }
                    }
                    let request_in_progress_clone = Arc::clone(&request_in_progress);
                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[4] {
                        println!("received connection closed notification");
                        request_in_progress_clone.lock().await.remove(&message_chunk.request_id);
                    }
                }
            }
        });

        tokio::select! {
            _ = process_message_future => {
                println!("Process Message Future ended!")
            },
            _ = ws_future => {
                println!("WS message handler ended!")
            },
            _ = ping_future => {
                println!("Ping handler ended!")
            }
        }
    }
}

fn build_response_info_chunk_body(header_map: &HeaderMap<HeaderValue>, http_version: Version, status_code: StatusCode) -> Vec<u8> {
    let version = match http_version {
        Version::HTTP_11 => "http/1.1",
        Version::HTTP_2 => "h2",
        _ => panic!("not support http version {:?}", http_version)
    };
    let mut result = format!("{} {} {}\r\n", version, status_code.as_str(), status_code.canonical_reason().unwrap());
    header_map.into_iter().for_each(|(key, value)| {
        result.push_str(format!("{}:{}\r\n", key.to_string().as_str(), value.to_str().unwrap().to_string().as_str()).as_str())
    });
    result.into_bytes()
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        // let result = add(2, 2);
        // assert_eq!(result, 4);
    }
}
