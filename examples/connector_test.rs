use std::env::set_var;
use reqwest::Client;
use vertx_http_gateway_rust_connector::{VertxHttpGatewayConnector};

#[tokio::main]
async fn main() {
    set_var("RUST_LOG", "debug");
    env_logger::init();
    let connector = VertxHttpGatewayConnector::builder(9090, "test-service".to_string(), 12345, "/register".to_string())
        .with_service_ssl(true)
        .with_service_host("localhost".to_string())
        .with_proxy_client(Client::builder().use_native_tls().connection_verbose(true).danger_accept_invalid_hostnames(true).build().unwrap())
        .build();

    connector.start().await;
}
