use super::*;
use futures::future::{Either, select};
use lact_schema::config::NvidiaPowerCapMode;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn pair() -> (DaemonClient, BufReader<tokio::net::UnixStream>) {
    let (client, server) = UnixStream::pair().unwrap();
    client.set_nonblocking(true).unwrap();
    server.set_nonblocking(true).unwrap();
    (
        DaemonClient::from_stream(client, false).unwrap(),
        BufReader::new(tokio::net::UnixStream::from_std(server).unwrap()),
    )
}

#[test]
fn cancelled_stats_reply_cannot_be_read_as_gpu_configuration() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (client, mut server) = pair();
                let mut line = String::new();
                let pending = Box::pin(client.get_device_stats("synthetic"));
                match select(pending, Box::pin(server.read_line(&mut line))).await {
                    Either::Right((result, pending)) => {
                        result.unwrap();
                        drop(pending); // The GUI aborts this polling task during a reload.
                    }
                    Either::Left(_) => panic!("GET completed before the server replied"),
                }
                assert!(line.contains("device_stats"));
                let stale = serde_json::to_string(&Response::Ok(DeviceStats::default())).unwrap();
                // These unrelated fields are accepted as a default configuration by serde.
                let decoded: Response<Option<GpuConfig>> = serde_json::from_str(&stale).unwrap();
                let Response::Ok(Some(default_config)) = decoded else {
                    panic!("expected a compatible but unrelated response");
                };
                assert_eq!(
                    default_config.nvidia_power_cap_mode,
                    NvidiaPowerCapMode::Nvml
                );
                assert!(default_config.nvidia_rail_controls.is_none());
                server
                    .write_all(format!("{stale}\n").as_bytes())
                    .await
                    .unwrap();
                let result = client.get_gpu_config("synthetic").await;
                assert!(
                    result.is_err(),
                    "cancelled stats reply became a default GPU configuration: {result:?}"
                );
            })
            .await
            .expect("synthetic cancellation test timed out");
        });
}

#[test]
fn completed_requests_keep_experimental_power_and_rail_configuration() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (client, mut server) = pair();
                let expected = GpuConfig {
                    nvidia_power_cap_mode: NvidiaPowerCapMode::Ioctl,
                    power_cap: Some(230.0),
                    nvidia_rail_controls: Some(lact_schema::NvidiaRailControls {
                        nvvdd: lact_schema::NvidiaRailSettings {
                            max_voltage_uv: Some(1125000),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                let client_flow = async {
                    client.get_device_stats("synthetic").await.unwrap();
                    assert_eq!(
                        client.get_gpu_config("synthetic").await.unwrap(),
                        Some(expected.clone())
                    );
                };
                let server_flow = async {
                    for (command, response) in [
                        (
                            "device_stats",
                            serde_json::to_string(&Response::Ok(DeviceStats::default())).unwrap(),
                        ),
                        (
                            "get_gpu_config",
                            serde_json::to_string(&Response::Ok(Some(&expected))).unwrap(),
                        ),
                    ] {
                        let mut line = String::new();
                        server.read_line(&mut line).await.unwrap();
                        assert!(line.contains(command));
                        server
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .unwrap();
                    }
                };
                futures::join!(client_flow, server_flow);
            })
            .await
            .expect("synthetic sequential requests timed out");
        });
}

#[test]
fn cancelled_poll_reconnects_before_loading_saved_rail_settings() {
    struct SocketDirectory(PathBuf);
    impl Drop for SocketDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0.join("daemon.sock"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = SocketDirectory(
        std::env::temp_dir().join(format!("lact-client-{}-{unique}", std::process::id())),
    );
    std::fs::create_dir(&directory.0).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let path = directory.0.join("daemon.sock");
                let listener = tokio::net::UnixListener::bind(&path).unwrap();
                let connection = UnixConnection::connect(&path).await.unwrap();
                let client = DaemonClient {
                    stream: Rc::new(Mutex::new(connection as Box<dyn DaemonConnection>)),
                    status_tx: broadcast::Sender::new(STATUS_MSG_CHANNEL_SIZE),
                    reconnect: true,
                    embedded: false,
                };
                let (old_socket, _) = listener.accept().await.unwrap();
                let mut old_server = BufReader::new(old_socket);
                let mut line = String::new();
                match select(
                    Box::pin(client.get_device_stats("synthetic")),
                    Box::pin(old_server.read_line(&mut line)),
                )
                .await
                {
                    Either::Right((result, pending)) => {
                        result.unwrap();
                        drop(pending);
                    }
                    Either::Left(_) => panic!("GET completed before its reply"),
                }
                let stale = serde_json::to_string(&Response::Ok(DeviceStats::default())).unwrap();
                old_server
                    .write_all(format!("{stale}\n").as_bytes())
                    .await
                    .unwrap();
                let expected: GpuConfig = serde_json::from_value(serde_json::json!({
                    "nvidia_power_cap_mode":"ioctl", "power_cap":230.0,
                    "nvidia_rail_controls":{"nvvdd":{"max_voltage_uv":1125000}}
                }))
                .unwrap();
                let client_flow = async {
                    assert_eq!(
                        client.get_gpu_config("synthetic").await.unwrap(),
                        Some(expected.clone())
                    );
                };
                let server_flow = async {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut new_server = BufReader::new(socket);
                    let mut request = String::new();
                    new_server.read_line(&mut request).await.unwrap();
                    assert!(request.contains("get_gpu_config"));
                    let reply = serde_json::to_string(&Response::Ok(Some(&expected))).unwrap();
                    new_server
                        .write_all(format!("{reply}\n").as_bytes())
                        .await
                        .unwrap();
                };
                futures::join!(client_flow, server_flow);
                // Keep the old peer alive through the new request: EOF must not
                // be what forces reconnection in this cancellation regression.
                drop(old_server);
            })
            .await
            .expect("synthetic reconnection timed out");
        });
}

async fn verify_optional_api_does_not_block_requests<S>(
    client: &DaemonClient,
    optional: &DaemonClient,
    mut main_server: BufReader<S>,
    mut optional_server: BufReader<S>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    assert!(!Rc::ptr_eq(&client.stream, &optional.stream));
    let mut line = String::new();
    let pending = match select(
        Box::pin(optional.get_device_api_info("synthetic")),
        Box::pin(optional_server.read_line(&mut line)),
    )
    .await
    {
        Either::Right((result, pending)) => {
            result.unwrap();
            pending
        }
        Either::Left(_) => panic!("optional API request completed before its reply"),
    };
    assert!(line.contains("device_api_info"));
    // The optional response is deliberately withheld until both foreground
    // requests finish. The confirmation endpoint here is only a fake peer.
    let foreground = async {
        client.ping().await.unwrap();
        client
            .confirm_pending_config(ConfirmCommand::Confirm)
            .await
            .unwrap();
    };
    let replies = async {
        for (command, reply) in [
            ("ping", serde_json::to_string(&Response::Ok(Pong)).unwrap()),
            (
                "confirm_pending_config",
                serde_json::to_string(&Response::Ok(())).unwrap(),
            ),
        ] {
            let mut line = String::new();
            main_server.read_line(&mut line).await.unwrap();
            assert!(line.contains(command));
            main_server
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        }
    };
    futures::join!(foreground, replies);
    let reply = serde_json::to_string(&Response::Ok(DeviceApiInfo::default())).unwrap();
    optional_server
        .write_all(format!("{reply}\n").as_bytes())
        .await
        .unwrap();
    pending.await.unwrap();
}

#[test]
fn independent_tcp_api_stream_preserves_endpoint_and_allows_ping_and_confirm() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let client = DaemonClient::connect_tcp(listener.local_addr().unwrap())
                    .await
                    .unwrap();
                let (main, _) = listener.accept().await.unwrap();
                let optional = client.independent_connection().await.unwrap();
                let (independent, _) = listener.accept().await.unwrap();
                verify_optional_api_does_not_block_requests(
                    &client,
                    &optional,
                    BufReader::new(main),
                    BufReader::new(independent),
                )
                .await;
            })
            .await
            .expect("independent TCP API stream blocked foreground requests");
        });
}

#[test]
fn independent_unix_api_stream_preserves_endpoint_and_allows_ping_and_confirm() {
    struct SocketPath(PathBuf);
    impl Drop for SocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = SocketPath(
        std::env::temp_dir().join(format!("lact-api-{}-{unique}.sock", std::process::id())),
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let listener = tokio::net::UnixListener::bind(&path.0).unwrap();
                let connection = UnixConnection::connect(&path.0).await.unwrap();
                let client = DaemonClient {
                    stream: Rc::new(Mutex::new(connection as Box<dyn DaemonConnection>)),
                    status_tx: broadcast::Sender::new(STATUS_MSG_CHANNEL_SIZE),
                    reconnect: true,
                    embedded: false,
                };
                let (main, _) = listener.accept().await.unwrap();
                let optional = client.independent_connection().await.unwrap();
                let (independent, _) = listener.accept().await.unwrap();
                verify_optional_api_does_not_block_requests(
                    &client,
                    &optional,
                    BufReader::new(main),
                    BufReader::new(independent),
                )
                .await;
            })
            .await
            .expect("independent Unix API stream blocked foreground requests");
        });
}

#[test]
fn unnamed_and_embedded_connections_keep_their_original_stream() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (mut client, _server) = pair();
            let optional = client.independent_connection().await.unwrap();
            assert!(Rc::ptr_eq(&client.stream, &optional.stream));
            client.embedded = true;
            let optional = client.independent_connection().await.unwrap();
            assert!(Rc::ptr_eq(&client.stream, &optional.stream));
        });
}

#[test]
fn unavailable_named_endpoint_does_not_fall_back_to_the_existing_stream() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = DaemonClient::connect_tcp(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (_server, _) = listener.accept().await.unwrap();
            drop(listener);
            assert!(client.independent_connection().await.is_err());
        });
}
