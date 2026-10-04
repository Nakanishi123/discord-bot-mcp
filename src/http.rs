use std::sync::Arc;

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

use crate::{config::Config, mcp::Mcp};

pub fn router(server: Mcp, config: &Config, cancellation: CancellationToken) -> Router {
    let mut transport = StreamableHttpServerConfig::default().enforce_origin_validation();
    transport.allowed_hosts.clone_from(&config.allowed_hosts);
    transport
        .allowed_origins
        .clone_from(&config.allowed_origins);
    transport.max_request_body_bytes = 65_536;
    transport.legacy_session_mode = false;
    transport.json_response = true;
    transport.cancellation_token = cancellation;
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        transport,
    );
    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::new(config.bearer_token.clone()),
            authenticate,
        ))
        .route("/healthz", get(|| async { StatusCode::OK }))
        .route("/readyz", get(|| async { StatusCode::OK }))
}

async fn authenticate(State(token): State<Arc<String>>, request: Request, next: Next) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let supplied = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, value)| value);
    if values.next().is_none()
        && supplied.is_some_and(|value| bool::from(value.as_bytes().ct_eq(token.as_bytes())))
    {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::{image::Images, test_support};

    fn post(body: &Value, authorization: Option<&str>) -> anyhow::Result<Request<Body>> {
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-03-26");
        if let Some(token) = authorization {
            request = request.header("authorization", token);
        }
        Ok(request.body(Body::from(body.to_string()))?)
    }

    async fn app() -> anyhow::Result<(Router, test_support::MockServer)> {
        let mock = test_support::mock_server(vec![]).await?;
        let config = test_support::config();
        let server = Mcp::new(test_support::discord(&mock)?, Images::new(&config)?);
        Ok((router(server, &config, CancellationToken::new()), mock))
    }

    #[tokio::test]
    async fn authenticates_all_mcp_methods_but_allows_health_probes() -> anyhow::Result<()> {
        let (app, mock) = app().await?;
        for method in ["GET", "POST", "DELETE"] {
            for token in [None, Some("Bearer incorrect"), Some("Basic abc")] {
                let mut request = post(&json!({}), token)?;
                *request.method_mut() = method.parse()?;
                let response = app.clone().oneshot(request).await?;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
            }
        }
        for path in ["/healthz", "/readyz"] {
            assert_eq!(
                app.clone()
                    .oneshot(Request::builder().uri(path).body(Body::empty())?)
                    .await?
                    .status(),
                StatusCode::OK
            );
        }
        assert!(mock.requests.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn negotiates_protocol_lists_tools_and_distinguishes_failures() -> anyhow::Result<()> {
        let (app, mock) = app().await?;
        let token = format!("Bearer {}", "a".repeat(32));
        let initialize = json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-03-26", "capabilities":{}, "clientInfo":{"name":"test", "version":"1"}
        }});
        let response = app
            .clone()
            .oneshot(post(&initialize, Some(&token))?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await?)?;
        assert_eq!(value["result"]["protocolVersion"], "2025-03-26");
        let response = app
            .clone()
            .oneshot(post(
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                Some(&token),
            )?)
            .await?;
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await?)?;
        assert_eq!(value["result"]["tools"].as_array().map(Vec::len), Some(6));
        for (arguments, error) in [
            (json!({"channel_id":"999","message_id":"1"}), false),
            (json!({"channel_id":"123","message_id":"0"}), true),
        ] {
            let response = app
                .clone()
                .oneshot(post(
                    &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"get_message","arguments":arguments}}),
                    Some(&token),
                )?)
                .await?;
            let value: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await?)?;
            if error {
                assert_eq!(value["error"]["code"], -32602);
            } else {
                assert_eq!(value["result"]["isError"], true);
            }
        }
        assert!(mock.requests.lock().await.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_untrusted_hosts_origins_and_oversized_requests() -> anyhow::Result<()> {
        let (app, _) = app().await?;
        let token = format!("Bearer {}", "a".repeat(32));
        for (header, value) in [("host", "evil.test"), ("origin", "https://evil.test")] {
            let mut request = post(&json!({}), Some(&token))?;
            request
                .headers_mut()
                .insert(axum::http::HeaderName::from_static(header), value.parse()?);
            assert_eq!(
                app.clone().oneshot(request).await?.status(),
                StatusCode::FORBIDDEN
            );
        }
        let request = post(&json!({"data":"a".repeat(70_000)}), Some(&token))?;
        assert_eq!(
            app.oneshot(request).await?.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        Ok(())
    }
    #[tokio::test]
    async fn lists_tools_with_current_protocol_metadata() -> anyhow::Result<()> {
        let (app, _) = app().await?;
        let token = format!("Bearer {}", "a".repeat(32));
        let mut request = post(
            &json!({"jsonrpc":"2.0", "id":1, "method":"tools/list", "params":{
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientInfo":{"name":"test", "version":"1"},
                    "io.modelcontextprotocol/clientCapabilities":{}
                }
            }}),
            Some(&token),
        )?;
        request
            .headers_mut()
            .insert("mcp-protocol-version", "2026-07-28".parse()?);
        request
            .headers_mut()
            .insert("mcp-method", "tools/list".parse()?);
        let response = app.oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await?)?;
        assert_eq!(value["result"]["tools"].as_array().map(Vec::len), Some(6));
        assert_eq!(value["result"]["ttlMs"], 0);
        Ok(())
    }
}
