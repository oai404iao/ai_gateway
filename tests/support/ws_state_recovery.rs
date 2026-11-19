use super::*;

async fn expiring_gateway(
    upstream: &MockResponsesWebSocket,
    idle_timeout: Duration,
) -> GatewayHarness {
    gateway_harness_with_controls(
        upstream,
        None,
        WebSocketControls {
            idle_timeout,
            ..WebSocketControls::default()
        },
    )
    .await
}

#[tokio::test]
async fn expired_response_id_rejects_the_delta_and_accepts_full_history() {
    let upstream = start_mock_upstream().await;
    let idle_timeout = Duration::from_millis(75);
    let gateway = expiring_gateway(&upstream, idle_timeout).await;
    let (mut socket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "expired-response",
    ))
    .await
    .unwrap();
    let first_input = json!({"role":"user", "content":"first turn"});
    let first = send_websocket_body(
        &mut socket,
        json!({
            "type":"response.create", "model":CLIENT_MODEL, "input":[first_input]
        }),
    )
    .await;
    tokio::time::sleep(idle_timeout * 2).await;
    let next_input = json!({"role":"user", "content":"follow-up"});
    let error = send_websocket_body(
        &mut socket,
        json!({
            "type":"response.create", "model":CLIENT_MODEL, "input":[next_input],
            "previous_response_id":completed_response_id(&first)
        }),
    )
    .await;
    assert_previous_response_not_found(&error);
    assert_eq!(upstream.handshakes().len(), 1);
    assert_eq!(upstream.requests().len(), 1);

    let history = json!([first_input, next_input]);
    let recovered = send_websocket_body(
        &mut socket,
        json!({
            "type":"response.create", "model":CLIENT_MODEL, "input":history
        }),
    )
    .await;
    assert_eq!(completed_response_id(&recovered), "resp-2");
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].connection, requests[1].connection);
    assert!(requests[1].body.get("previous_response_id").is_none());
    assert_eq!(requests[1].body["input"], history);
}

#[tokio::test]
#[ignore = "requires a local Codex CLI; isolated synthetic credentials and loopback mocks only"]
async fn codex_cli_resends_full_tool_history_after_idle_expiry() {
    let upstream = start_mock_upstream_with_script(None, true).await;
    let gateway = expiring_gateway(&upstream, Duration::from_micros(1)).await;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("marker.txt"),
        "expiry-fixture-marker\n",
    )
    .unwrap();
    let prompt = "Read marker.txt using the shell and reply E2E_TOOL_OK.";
    let answer = run_loopback_codex(gateway.server.address, directory.path(), prompt).await;
    assert_eq!(answer.trim(), "E2E_TOOL_OK");
    let logs = gateway.logs.events();
    let [.., initial, miss, retried] = logs.as_slice() else {
        panic!("missing WebSocket recovery sequence");
    };
    assert_eq!(initial.response_status_code, Some(200));
    assert_eq!(miss.response_status_code, Some(404));
    assert_eq!(
        miss.error_code.as_deref(),
        Some("previous_response_not_found")
    );
    assert_eq!(retried.response_status_code, Some(200));
    assert!(
        logs.iter()
            .all(|e| e.request_protocol == RequestProtocol::WebSocket)
    );
    let requests: Vec<_> = upstream
        .requests()
        .into_iter()
        .filter(|r| r.body["generate"] != false)
        .collect();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].connection, requests[1].connection);
    assert!(
        requests
            .iter()
            .all(|r| r.body.get("previous_response_id").is_none())
    );
    let input = requests[1].body["input"].as_array().unwrap();
    assert!(
        input
            .iter()
            .any(|i| i["role"] == "user" && i["content"].to_string().contains(prompt))
    );
    assert!(
        input
            .iter()
            .any(|i| i["type"] == "function_call" && i["call_id"] == "call_system_e2e")
    );
    assert!(input.iter().any(|i| i["type"] == "function_call_output"
        && i["call_id"] == "call_system_e2e"
        && i["output"].to_string().contains("expiry-fixture-marker")));
}
