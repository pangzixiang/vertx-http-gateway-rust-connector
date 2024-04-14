# vertx-http-gateway-rust-connector
[![CRATES](https://img.shields.io/crates/v/vertx-http-gateway-rust-connector.svg)](https://crates.io/crates/vertx-http-gateway-rust-connector)
## What is it
Rust implementation for [vertx-http-gateway-connector](https://github.com/pangzixiang/vertx-http-gateway)
## How to use
```rust
use vertx_http_gateway_rust_connector::{ConnectorOptions, VertxHttpGatewayConnector};

#[tokio::main]
async fn main() {
    env_logger::init();
    let connector = VertxHttpGatewayConnector::new(ConnectorOptions {
        register_host: "localhost".to_string(),
        register_port: 9090,
        service_host: None,
        service_name: "test-service".to_string(),
        service_port: 12345,
        register_path: "/register".to_string(),
        instance: None
    });

    connector.start().await;
}
```
