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
