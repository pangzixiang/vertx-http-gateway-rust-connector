use vertx_http_gateway_rust_connector::{ConnectorOptions, VertxHttpGatewayConnector};

#[tokio::main]
async fn main() {
    let connector = VertxHttpGatewayConnector::new(ConnectorOptions {
        register_host: "localhost".to_string(),
        register_port: 9090,
        service_host: "localhost".to_string(),
        service_name: "test-service".to_string(),
        service_port: 12345,
        register_path: "/register".to_string(),
    });
    
    connector.start().await;
}
