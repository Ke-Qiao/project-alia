// Diagnostic sketch only; do not add to default tests because cross-thread address reclaim can race.

    #[tokio::test]
    async fn original_setup_close_is_proven_even_when_its_address_is_reused() {
        let listener = bind_setup_listener(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut original_socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        let (dropped, mut listener_closed) = tokio::sync::oneshot::channel();
        let original = SetupListenerProbe {
            listener: Some(super::super::runtime::HttpListener {
                listener,
                connections: Connections::new(),
            }),
            dropped: Some(dropped),
        };
        let replacement = tokio::net::TcpSocket::new_v4().unwrap();
        replacement.set_reuseaddr(true).unwrap();
        drop(accepted);
        drop(original);
        // Reclaim the address synchronously; no sleep or competing test is required.
        replacement.bind(address).unwrap();
        let replacement = replacement.listen(1).unwrap();
        let connected = tokio::net::TcpStream::connect(address).await.unwrap();
        let (_, peer) = replacement.accept().await.unwrap();
        assert_eq!(peer, connected.local_addr().unwrap());
        assert_setup_released(&mut listener_closed, &mut original_socket).await;
    }
