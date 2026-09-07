use super::*;
use std::sync::mpsc;
use std::thread;

#[derive(Debug, PartialEq)]
enum Event {
    Stalled,
    Enqueued(Vec<i16>),
    Output(Vec<i16>),
    Stopped,
}

struct TestSink {
    buffer: PlaybackBuffer,
    stalled: bool,
    reported_stall: bool,
    events: mpsc::Sender<Event>,
}

impl PlaybackSink for TestSink {
    fn buffered_ms(&self) -> u64 {
        self.buffer.len() as u64 // The fake output clock is 1 kHz.
    }

    fn enqueue(&mut self, block: AudioBlock) -> Result<()> {
        let samples = block.samples().to_vec();
        if self.buffer.append(block.generation, samples.clone()) {
            self.events.send(Event::Enqueued(samples)).unwrap();
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.buffer.clear();
        self.stalled = false;
        self.events.send(Event::Stopped).unwrap();
        Ok(())
    }

    fn pump(&mut self) -> Result<()> {
        if self.stalled {
            if !self.reported_stall {
                self.events.send(Event::Stalled).unwrap();
                self.reported_stall = true;
            }
        } else if self.buffer.len() > 0 {
            let output = (0..self.buffer.len())
                .map(|_| self.buffer.next_sample())
                .collect();
            self.events.send(Event::Output(output)).unwrap();
        }
        Ok(())
    }
}

fn worker(
    receiver: PlaybackReceiver,
    initially_full: bool,
) -> (thread::JoinHandle<Result<()>>, mpsc::Receiver<Event>) {
    let (events, received) = mpsc::channel();
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    if initially_full {
        buffer.append(0, vec![9; 100]);
    }
    let handle = thread::spawn(move || {
        let mut sink = TestSink {
            buffer,
            stalled: initially_full,
            reported_stall: false,
            events,
        };
        run(receiver, &mut sink, 10)
    });
    (handle, received)
}

fn next(events: &mpsc::Receiver<Event>) -> Event {
    // A deadlock guard, not a claim about real sound-card latency.
    events.recv_timeout(Duration::from_secs(1)).unwrap()
}

#[test]
fn full_device_buffer_and_full_media_queue_do_not_delay_stop() {
    let (sender, receiver) = channel(1);
    sender.play(&[1], 1000).unwrap(); // The only media slot is full.
    let (handle, events) = worker(receiver, true);
    assert_eq!(next(&events), Event::Stalled); // The device does not drain.

    sender.stop();
    assert_eq!(next(&events), Event::Stopped);
    sender.shutdown();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn stop_discards_queued_media_and_allows_new_playback() {
    let (sender, receiver) = channel(2);
    sender.play(&[1], 1000).unwrap();
    sender.play(&[2], 1000).unwrap();
    sender.stop();
    sender.play(&[3, 4], 1000).unwrap();

    let (handle, events) = worker(receiver, false);
    assert_eq!(next(&events), Event::Stopped);
    assert_eq!(next(&events), Event::Enqueued(vec![3, 4]));
    assert_eq!(next(&events), Event::Output(vec![3, 4]));
    sender.shutdown();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn stop_invalidates_rendered_audio_and_a_block_already_in_flight() {
    let (sender, receiver) = channel(1);
    let mut buffer = PlaybackBuffer::new(receiver.generation());
    sender.play(&[1], 1000).unwrap();
    let Some(PlaybackAction::Play(in_flight)) = receiver.take(true) else {
        panic!("expected submitted audio");
    };
    buffer.append(in_flight.generation, vec![7, 8]);

    sender.stop();
    // The callback rejects old audio even before the worker handles Stop.
    assert_eq!(buffer.next_sample(), 0);
    assert!(!buffer.append(in_flight.generation, in_flight.samples().to_vec()));
    assert_eq!(buffer.next_sample(), 0);

    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    sender.play(&[5], 1000).unwrap();
    let Some(PlaybackAction::Play(new)) = receiver.take(true) else {
        panic!("expected new-generation audio");
    };
    assert!(buffer.append(new.generation, new.samples().to_vec()));
    assert_eq!(buffer.next_sample(), 5);
}

#[test]
fn disconnect_shuts_down_without_waiting_for_the_device_to_drain() {
    let (sender, receiver) = channel(1);
    sender.play(&[1], 1000).unwrap();
    let (handle, events) = worker(receiver, true);
    assert_eq!(next(&events), Event::Stalled);
    drop(sender);
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn disconnect_wakes_an_idle_worker() {
    let (sender, receiver) = channel(1);
    let (handle, events) = worker(receiver, false);
    drop(sender);
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
}

#[test]
fn device_failure_wakes_worker_and_rejects_further_media() {
    let (sender, receiver) = channel(1);
    let fail = receiver.device_failed();
    let (handle, events) = worker(receiver, false);
    fail();
    assert_eq!(next(&events), Event::Stopped);
    handle.join().unwrap().unwrap();
    assert!(sender.play(&[1], 1000).is_err());
}

#[test]
fn receiver_failure_releases_a_producer_when_the_queue_is_full() {
    let (sender, receiver) = channel(1);
    sender.play(&[1], 1000).unwrap();
    let (result_tx, result_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        result_tx.send(sender.play(&[2], 1000)).unwrap();
    });
    drop(receiver);
    assert!(
        result_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err()
    );
    handle.join().unwrap();
}

#[test]
fn repeated_stop_coalesces_without_a_control_backlog() {
    let (sender, receiver) = channel(1);
    for _ in 0..1000 {
        sender.stop();
    }
    assert!(matches!(receiver.take(false), Some(PlaybackAction::Stop)));
    assert!(receiver.take(false).is_none());
    sender.play(&[1], 1000).unwrap();
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Play(_))));
}

#[test]
fn stop_retires_the_remainder_of_a_partly_consumed_clip() {
    let (sender, receiver) = channel(1);
    sender.play(&vec![1; 1000], 1000).unwrap();
    let Some(PlaybackAction::Play(first)) = receiver.take(true) else {
        panic!("expected first block");
    };
    assert_eq!(first.samples(), &[1; 20]);
    sender.stop();
    sender.play(&[2], 1000).unwrap();
    assert!(matches!(receiver.take(true), Some(PlaybackAction::Stop)));
    let Some(PlaybackAction::Play(fresh)) = receiver.take(true) else {
        panic!("expected fresh clip");
    };
    assert_eq!(fresh.samples(), &[2]);
    assert!(receiver.take(true).is_none());
}

#[test]
fn worker_blocks_preserve_resampling_phase_and_duration() {
    // 11025 Hz does not divide into the worker's 20 ms source blocks.
    let pcm: Vec<i16> = (0..11025).collect();
    for output_rate in [8000, 48000] {
        let (sender, receiver) = channel(1);
        sender.play(&pcm, 11025).unwrap();
        let mut rendered = Vec::new();
        while let Some(PlaybackAction::Play(block)) = receiver.take(true) {
            rendered.extend(block.render(output_rate, 2));
        }
        assert_eq!(rendered.len(), output_rate as usize * 2);
        for (index, stereo) in rendered.chunks_exact(2).enumerate() {
            let expected = (index as f64 * (11025.0 / output_rate as f64))
                .min(11024.0)
                .round() as i16;
            assert_eq!(stereo, &[expected, expected]);
        }
    }
}

#[test]
fn submillisecond_device_tail_is_not_idle() {
    assert_eq!(super::super::playback_buffered_ms(0, 48000, 2), 0);
    assert_eq!(super::super::playback_buffered_ms(2, 48000, 2), 1);
}
