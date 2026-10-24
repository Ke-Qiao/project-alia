use super::*;

#[tokio::test]
async fn capture_owner_reports_exit_and_requires_stop_before_restart() {
    let capture = CpalAudioCapture::new(
        // An invalid device proves that health never probes or reopens hardware.
        AudioConfig {
            capture_device: "missing-exclusive-device".into(),
            ..Default::default()
        },
        AudioVolumeState::new(Default::default()),
    );
    let bus = AudioBus::new(8);
    let (fail, failed) = mpsc::channel();
    capture
        .start_with(&bus, move |_, _, _, ready| {
            ready.send(Ok(())).unwrap();
            failed.recv().unwrap();
            bail!("injected backend failure")
        })
        .unwrap();
    assert!(capture.is_available());
    assert_eq!(capture.health_check().await, ModuleHealth::Healthy);
    capture
        .start_with(&bus, |_, _, _, _| panic!("must not reopen active device"))
        .unwrap();
    let stopped = bus.capture_stopped().unwrap();
    fail.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), stopped.cancelled())
        .await
        .unwrap();
    assert!(!capture.is_available());
    assert_eq!(capture.health_check().await, ModuleHealth::Unhealthy);
    assert!(
        capture
            .start_with(&bus, |_, _, _, _| panic!("must join old owner first"))
            .is_err()
    );
    capture.stop_capture().unwrap();
    capture
        .start_with(&bus, |_, _, control, ready| {
            ready.send(Ok(())).unwrap();
            control.recv().unwrap();
            Ok(())
        })
        .unwrap();
    assert!(capture.is_available());
    assert!(!bus.capture_stopped().unwrap().is_cancelled());
    capture.stop_capture().unwrap();
    assert!(!capture.is_available());
    assert!(bus.capture_stopped().unwrap().is_cancelled());
}

#[test]
fn failed_start_is_joined_and_can_retry() {
    let capture = CpalAudioCapture::new(
        AudioConfig::default(),
        AudioVolumeState::new(Default::default()),
    );
    let bus = AudioBus::new(8);
    assert!(
        capture
            .start_with(&bus, |_, _, _, ready| {
                ready.send(Err("injected startup failure".into())).unwrap();
                Ok(())
            })
            .is_err()
    );
    assert!(!capture.is_available());
    assert!(bus.capture_stopped().unwrap().is_cancelled());
    capture
        .start_with(&bus, |_, _, control, ready| {
            ready.send(Ok(())).unwrap();
            control.recv().unwrap();
            Ok(())
        })
        .unwrap();
    capture.stop_capture().unwrap();
}
