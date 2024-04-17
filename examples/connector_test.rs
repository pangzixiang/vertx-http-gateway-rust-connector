use std::env::set_var;
use reqwest::Client;
use tokio_tungstenite::Connector;
use vertx_http_gateway_rust_connector::{VertxHttpGatewayConnector};

#[tokio::main]
async fn main() {
    set_var("RUST_LOG", "debug");
    env_logger::init();
    let tls = native_tls::TlsConnector::builder().danger_accept_invalid_hostnames(true).danger_accept_invalid_certs(true).build().unwrap();
    let connector = VertxHttpGatewayConnector::builder(9090, "test-service".to_string(), 12345, "/register".to_string())
        .with_service_ssl(true)
        .with_service_host("localhost".to_string())
        .with_register_ssl(true)
        .with_register_tls_connector(Connector::NativeTls(tls.clone()))
        .with_proxy_client(Client::builder().use_preconfigured_tls(tls.clone()).no_proxy().build().unwrap())
        .build();

    connector.start().await;
}
