use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use bytes::Bytes;
use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, trace};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Body, Client, Method, StatusCode, Version};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use tokio::time;
use tokio_tungstenite::{client_async_tls_with_config};
use tokio_tungstenite::tungstenite::{Message};
use tokio_tungstenite::Connector;

use crate::message_chunk::MessageChunk;
use crate::request_message_info_chunk_body::RequestMessageInfoChunkBody;

mod message_chunk;
mod request_message_info_chunk_body;

const CHUNK_TYPE_VEC: [i8; 5] = [-2, 0, 1, 2, 3];

pub struct VertxHttpGatewayConnector {
    register_host: String,
    register_port: u16,
    register_path: String,
    register_ssl: bool,
    register_tls_connector: Option<Connector>,
    service_name: String,
    service_host: String,
    service_port: u16,
    instance: u8,
    proxy_client: Client,
    service_ssl: bool,
}

struct ConnectorOptions {
    register_host: String,
    register_port: u16,
    register_tls_connector: Option<Connector>,
    service_host: String,
    service_name: String,
    service_port: u16,
    service_ssl: bool,
    register_path: String,
    instance: u8,
    proxy_client: Client,
}

pub struct ConnectorOptionsBuilder {
    options: ConnectorOptions,
}

impl ConnectorOptionsBuilder {
    pub fn new(register_port: u16, service_name: String, service_port: u16, register_path: String) -> ConnectorOptionsBuilder {
        ConnectorOptionsBuilder {
            options: ConnectorOptions {
                register_host: "localhost".to_string(),
                register_port,
                register_tls_connector: None,
                service_host: "localhost".to_string(),
                service_name,
                service_port,
                service_ssl: false,
                register_path,
                instance: 2,
                proxy_client: Client::builder().build().expect("Failed to build reqwest client")
            }
        }
    }

    pub fn with_register_host(mut self, register_host: String) -> ConnectorOptionsBuilder {
        self.options.register_host = register_host;
        self
    }

    pub fn with_service_host(mut self, service_host: String) -> ConnectorOptionsBuilder {
        self.options.service_host = service_host;
        self
    }

    pub fn with_service_ssl(mut self, service_ssl: bool) -> ConnectorOptionsBuilder {
        self.options.service_ssl = service_ssl;
        self
    }

    pub fn with_instance(mut self, instance: u8) -> ConnectorOptionsBuilder {
        self.options.instance = instance;
        self
    }

    pub fn with_register_tls_connector(mut self, register_tls_connector: Connector) -> ConnectorOptionsBuilder {
        self.options.register_tls_connector = Option::from(register_tls_connector);
        self
    }

    pub fn with_proxy_client(mut self, proxy_client: Client) -> ConnectorOptionsBuilder {
        self.options.proxy_client = proxy_client;
        self
    }

    pub fn build(self) -> VertxHttpGatewayConnector {
        VertxHttpGatewayConnector {
            register_host: self.options.register_host,
            register_port: self.options.register_port,
            register_path: self.options.register_path,
            register_ssl: self.options.register_tls_connector.is_some(),
            register_tls_connector: self.options.register_tls_connector,
            service_host: self.options.service_host,
            service_port: self.options.service_port,
            service_name: self.options.service_name,
            service_ssl: self.options.service_ssl,
            instance: self.options.instance,
            proxy_client: self.options.proxy_client
        }
    }
}

impl VertxHttpGatewayConnector {
    pub fn builder(register_port: u16, service_name: String, service_port: u16, register_path: String) -> ConnectorOptionsBuilder {
        ConnectorOptionsBuilder::new(register_port, service_name, service_port, register_path)
    }

    async fn connect(proxy_client: Client, connection_url: String, tcp_stream: TcpStream, register_tls_connector: Option<Connector>, service_host: String, service_port: u16, service_ssl: bool) -> Result<(), String> {
        let url = connection_url.clone();
        info!("Connector start to connect to {}", url);
        let ws_connect_result = client_async_tls_with_config(connection_url, tcp_stream, None, register_tls_connector).await;
        // let ws_connect_result = connect_async(connection_url).await;

        if let Err(e) = ws_connect_result {
            let err = format!("Failed to connect to {} due to {}", url, e.to_string());
            return Err(err);
        }
        let (ws_stream, _) = ws_connect_result.unwrap();
        info!("Connector succeeded to connect {}!", url);

        let (mut ws_write, mut ws_read) = ws_stream.split();

        let (ws_tx, mut ws_rx) = mpsc::unbounded::<Result<Vec<u8>, Box<dyn Error + Send + Sync>>>();

        let ws_future = tokio::spawn(async move {
            while let Some(message) = ws_rx.next().await {
                let b = message.unwrap();
                if b == "ping".as_bytes().to_vec() {
                    ws_write.send(Message::Ping(b)).await.expect("Failed to send ping to ws");
                } else {
                    ws_write.send(Message::Binary(b)).await.expect("Failed to send binary message to ws");
                }
            }
        });

        let ws_tx_ping = ws_tx.clone();
        let ping_future = tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                trace!("send ping to gateway server");
                ws_tx_ping.unbounded_send(Ok("ping".as_bytes().to_vec())).expect("Failed to send ping to ws_tx channel");
            }
        });
        let request_in_progress = Arc::new(Mutex::new(HashMap::new()));

        let process_message_future = tokio::spawn(async move {
            let mut body_sender = None;
            while let Some(msg) = ws_read.next().await {
                let msg = msg.expect("Failed to receive message from gateway server");
                let ws_tx_clone = ws_tx.clone();
                let request_in_progress_clone = Arc::clone(&request_in_progress);
                if msg.is_close() {}
                if msg.is_ping() {
                    trace!("received ping from server");
                }
                if msg.is_text() {
                    trace!("received text from server: {}", msg.clone().into_text().unwrap());
                }
                if msg.is_pong() {
                    trace!("received pong from server");
                }
                if msg.is_binary() {
                    let message_chunk = MessageChunk::new(msg.clone());
                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[1] {
                        let request_message_info_chunk_body = RequestMessageInfoChunkBody::new(String::from_utf8(message_chunk.chunk_body.to_vec()).unwrap());
                        info!("[{}] start to handle proxy for {} {}", message_chunk.request_id ,request_message_info_chunk_body.http_method, request_message_info_chunk_body.uri);

                        request_in_progress_clone.lock().await.insert(message_chunk.request_id, true);

                        let url_prefix = match service_ssl {
                            true => "https://",
                            false => "http://"
                        };

                        let mut request_builder = proxy_client.request(
                            Method::from_bytes(request_message_info_chunk_body.http_method.as_bytes()).expect(&format!("unable to parse request method {}", request_message_info_chunk_body.http_method)),
                            format!("{}{}:{}{}", url_prefix, service_host, service_port, request_message_info_chunk_body.uri));

                        for (key, value) in request_message_info_chunk_body.headers {
                            request_builder = request_builder.header(key, value);
                        }

                        if request_message_info_chunk_body.http_method != "GET" {
                            let (sender, recv) = mpsc::unbounded::<Result<Bytes, Box<dyn Error + Send + Sync>>>();
                            body_sender = Some(sender);
                            request_builder = request_builder.body(Body::wrap_stream(recv));
                        }

                        tokio::spawn(async move {
                            let response_result = request_builder.send().await;

                            if let Err(e) = response_result {
                                let mut end_buffer = CHUNK_TYPE_VEC[0].to_be_bytes().to_vec();
                                end_buffer.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                                end_buffer.append(&mut e.to_string().into_bytes());
                                ws_tx_clone.unbounded_send(Ok(end_buffer)).expect("failed to send response body end chunk to upstream");
                                info!("[{}] failed to handle proxy for {} {} due to {}", message_chunk.request_id ,request_message_info_chunk_body.http_method, request_message_info_chunk_body.uri, e);
                            } else {
                                let response = response_result.unwrap();

                                debug!("[{}] proxied response status: {}", message_chunk.request_id, response.status());

                                let response_message_info_chunk_body = build_response_info_chunk_body(response.headers(), response.version(), response.status());
                                let mut info_chunk_bytes: Vec<u8> = CHUNK_TYPE_VEC[1].to_be_bytes().to_vec();
                                info_chunk_bytes.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                                info_chunk_bytes.append(&mut response_message_info_chunk_body.clone());
                                ws_tx_clone.unbounded_send(Ok(info_chunk_bytes)).expect("failed to send first info response chunk");

                                let mut stream = response.bytes_stream();

                                while let Some(b) = stream.next().await {
                                    if let Some(_) = request_in_progress_clone.lock().await.get(&message_chunk.request_id) {
                                        let mut body_buffer = CHUNK_TYPE_VEC[2].to_be_bytes().to_vec();
                                        body_buffer.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                                        body_buffer.append(&mut b.unwrap().to_vec());
                                        trace!("[{}] received proxied response body chunk: {:?}", message_chunk.request_id, body_buffer);
                                        ws_tx_clone.unbounded_send(Ok(body_buffer)).expect("failed to send response body chunk to upstream");
                                    } else {
                                        break;
                                    }
                                }

                                let mut end_buffer = CHUNK_TYPE_VEC[3].to_be_bytes().to_vec();
                                end_buffer.append(&mut message_chunk.request_id.clone().to_be_bytes().to_vec());
                                ws_tx_clone.unbounded_send(Ok(end_buffer)).expect("failed to send response body end chunk to upstream");

                                info!("[{}] succeeded to handle proxy for {} {}", message_chunk.request_id ,request_message_info_chunk_body.http_method, request_message_info_chunk_body.uri);
                            }
                        });
                    }

                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[2] {
                        debug!("[{}] received request body chunk from upstream: {:?}", message_chunk.request_id ,message_chunk.chunk_body);
                        if let Some(sender) = body_sender.clone() {
                            sender.unbounded_send(Ok(message_chunk.chunk_body)).expect("failed to send request body chunk to downstream");
                        }
                    }

                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[3] {
                        debug!("[{}] received request end notification from upstream", message_chunk.request_id);
                        if let Some(sender) = body_sender.clone() {
                            trace!("[{}] send body end notification to downstream", message_chunk.request_id);
                            sender.close_channel();
                        }
                    }
                    let request_in_progress_clone = Arc::clone(&request_in_progress);
                    if message_chunk.chunk_type == CHUNK_TYPE_VEC[4] {
                        debug!("[{}] received connection closed notification from upstream", message_chunk.request_id);
                        request_in_progress_clone.lock().await.remove(&message_chunk.request_id);
                    }
                }
            }
        });

        tokio::select! {
            _ = process_message_future => {
                error!("Process Message Future ended!");
                Err(String::from("Connector message processor stopped unexpectedly!"))
            },
            _ = ws_future => {
                error!("Websocket message handler ended!");
                Err(String::from("Websocket message handler stopped unexpectedly!"))
            },
            _ = ping_future => {
                error!("Ping handler ended!");
                Err(String::from("Websocket ping handler stopped unexpectedly!"))
            }
        }
    }

    pub async fn start(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            let service_port = self.service_port.clone();
            let register_host = self.register_host.clone();
            let register_port = self.register_port.clone();
            let instance = self.instance.clone();
            let service_ssl = self.service_ssl.clone();
            let mut set = JoinSet::new();
            let ws_url_prefix = match self.register_ssl {
                true => "wss://",
                false => "ws://"
            };
            let connection_url = format!("{}{}:{}{}?serviceName={}&servicePort={}",ws_url_prefix, register_host, register_port, self.register_path, self.service_name, service_port);
            for _ in 0..instance {
                let proxy_client = self.proxy_client.clone();
                let service_host = self.service_host.clone();
                let connector = self.register_tls_connector.clone();
                let tcp_stream = TcpStream::connect(format!("{}:{}", register_host, register_port)).await.expect("Failed to start tcp connection");
                set.spawn(Self::connect(proxy_client, format!("{}&instance={}", connection_url, uuid::Uuid::new_v4().to_string()), tcp_stream, connector, service_host, service_port, service_ssl));
            }
            while let Some(res) = set.join_next().await {
                if let Err(e) = res.unwrap() {
                    error!("Connection failed: {}, Connector will retry connection...", e.to_string());
                    interval.tick().await;
                    break;
                }
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
