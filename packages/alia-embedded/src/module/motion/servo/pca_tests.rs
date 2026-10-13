use super::*;
use std::cell::{Cell, RefCell};
use std::io;
use std::rc::Rc;

const MODE1: usize = 0x00;
const PRESCALE: usize = 0xfe;
const RESTART: u8 = 0x80;
const AI: u8 = 0x20;
const SLEEP: u8 = 0x10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Sleep,
    Prescale,
    Wake,
    Restart,
    Pwm,
    ReadMode,
    ReadPrescale,
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    Before,
    After,
    Short(usize),
}

// Models only the documented register rules exercised by the production driver.
struct Registers {
    values: [u8; 256],
    pointer: u8,
    writes: Vec<Vec<u8>>,
    cancelled: Rc<Cell<bool>>,
    cancel_after_sleep: bool,
    fail_prescale_once: bool,
    cancel_on: Option<(Event, usize)>,
    failures: Vec<(Event, usize, Failure)>,
    counts: [usize; 7],
    address: u16,
    events: Rc<RefCell<Vec<(u16, Event)>>>,
    pwm_running: bool,
}

impl Registers {
    fn new(mode: u8, prescale: u8) -> Self {
        let mut values = [0; 256];
        values[MODE1] = mode;
        values[PRESCALE] = prescale;
        // Existing channel contents must survive initialization failures.
        values[0x08] = 0x80;
        values[0x09] = 0x01;
        Self {
            values,
            pointer: 0,
            writes: Vec::new(),
            cancelled: Rc::new(Cell::new(false)),
            cancel_after_sleep: false,
            fail_prescale_once: false,
            cancel_on: None,
            failures: Vec::new(),
            counts: [0; 7],
            address: 0x40,
            events: Rc::new(RefCell::new(Vec::new())),
            pwm_running: mode & SLEEP == 0,
        }
    }

    fn device(self) -> Pca9685Device<Self> {
        model_device(self)
    }

    fn event(&mut self, event: Event) -> Option<Failure> {
        self.events.borrow_mut().push((self.address, event));
        let count = &mut self.counts[event as usize];
        *count += 1;
        self.failures
            .iter()
            .find(|&&(trigger, occurrence, _)| trigger == event && occurrence == *count)
            .map(|&(_, _, failure)| failure)
    }

    fn cancel_at(&self, event: Event) {
        if self.cancel_on == Some((event, self.counts[event as usize])) {
            self.cancelled.set(true);
        }
    }

    fn data_writes(&self) -> usize {
        self.writes.iter().filter(|bytes| bytes.len() > 1).count()
    }

    fn assert_original(&self, mode: u8, prescale: u8) {
        // RESTART is a status/trigger bit; only the stable configuration is restored.
        assert_eq!(self.values[MODE1] & !RESTART, mode & !RESTART);
        if mode & SLEEP == 0 {
            assert_eq!(self.values[MODE1] & RESTART, 0);
        }
        assert_eq!(self.values[PRESCALE], prescale);
        self.assert_no_pwm_writes();
    }

    fn assert_no_pwm_writes(&self) {
        assert!(
            self.writes
                .iter()
                .all(|bytes| { bytes.len() == 1 || !(0x06..=0x45).contains(&bytes[0]) })
        );
    }
}

fn model_device<Io>(file: Io) -> Pca9685Device<Io> {
    Pca9685Device {
        file,
        address: 0x40,
        frequency_hz: 50,
        min_pulse_us: 500,
        max_pulse_us: 2500,
    }
}

impl Read for Registers {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let event = match self.pointer as usize {
            MODE1 => Event::ReadMode,
            PRESCALE => Event::ReadPrescale,
            _ => panic!("unexpected model register read"),
        };
        if self.event(event).is_some() {
            return Err(io::Error::other(format!("injected {event:?} failure")));
        }
        for byte in out.iter_mut() {
            *byte = self.values[self.pointer as usize];
            if self.values[MODE1] & AI != 0 {
                self.pointer = self.pointer.wrapping_add(1);
            }
        }
        self.cancel_at(event);
        Ok(out.len())
    }
}

impl Write for Registers {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.push(bytes.to_vec());
        assert!(!bytes.is_empty());
        let event = if bytes.len() == 1 {
            None
        } else {
            Some(match bytes[0] as usize {
                MODE1 if bytes[1] & SLEEP != 0 => Event::Sleep,
                MODE1 if bytes[1] & RESTART != 0 => Event::Restart,
                MODE1 => Event::Wake,
                PRESCALE => Event::Prescale,
                0x06..=0x45 => Event::Pwm,
                _ => panic!("unexpected model register write"),
            })
        };
        let failure = event.and_then(|event| self.event(event));
        if matches!(failure, Some(Failure::Before)) {
            return Err(io::Error::other(format!(
                "injected {event:?} failure before write"
            )));
        }
        if bytes.len() > 1 && bytes[0] as usize == PRESCALE && self.fail_prescale_once {
            self.fail_prescale_once = false;
            return Err(io::Error::other("injected prescale failure"));
        }
        let count = match failure {
            Some(Failure::Short(count)) => count.min(bytes.len()),
            _ => bytes.len(),
        };
        if count == 0 {
            return Ok(0);
        }
        self.pointer = bytes[0];
        for &value in &bytes[1..count] {
            let reg = self.pointer as usize;
            match reg {
                MODE1 => {
                    let old = self.values[MODE1];
                    // RESTART is write-one-to-clear, not a saved configuration bit.
                    let mut restart = old & RESTART;
                    if old & SLEEP == 0 && value & SLEEP != 0 && self.pwm_running {
                        restart = RESTART;
                        self.pwm_running = false;
                    }
                    if value & RESTART != 0 && value & SLEEP == 0 {
                        self.pwm_running |= restart != 0;
                        restart = 0;
                    }
                    self.values[MODE1] = (value & !RESTART) | restart | (old & PCA_EXTCLK);
                    if value & SLEEP != 0 && self.cancel_after_sleep {
                        self.cancel_after_sleep = false;
                        self.cancelled.set(true);
                    }
                }
                PRESCALE if self.values[MODE1] & SLEEP == 0 => {}
                _ => {
                    self.values[reg] = value;
                    if (0x06..=0x45).contains(&reg) {
                        self.values[MODE1] &= !RESTART;
                        self.pwm_running = self.values[MODE1] & SLEEP == 0;
                    }
                }
            }
            if self.values[MODE1] & AI != 0 {
                self.pointer = self.pointer.wrapping_add(1);
            }
        }
        if let Some(event) = event {
            self.cancel_at(event);
        }
        if matches!(failure, Some(Failure::After)) {
            return Err(io::Error::other(format!(
                "injected {event:?} failure after write"
            )));
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn pca_initialization_enables_increment_for_real_pwm_burst() {
    let mut device = Registers::new(0x01, 0x1e).device();
    device.initialize(&|| false).unwrap();
    device.set_pwm(2, 0x0123, 0x0345).unwrap();
    eprintln!(
        "PCA model: mode={:#04x}, pwm={:?}",
        device.file.values[MODE1],
        &device.file.values[0x0e..0x12]
    );
    assert_eq!(&device.file.values[0x0e..0x12], &[0x23, 0x01, 0x45, 0x03]);
    assert_ne!(device.file.values[MODE1] & AI, 0);
}

#[test]
fn pca_cancellation_after_sleep_restores_original_running_configuration() {
    let original_mode = 0x0b;
    let original_prescale = 0x21;
    let mut registers = Registers::new(original_mode, original_prescale);
    registers.cancel_after_sleep = true;
    let cancelled = Rc::clone(&registers.cancelled);
    let original_pwm = registers.values[0x06..0x46].to_vec();
    let mut device = registers.device();
    let error = device.initialize(&|| cancelled.get()).unwrap_err();
    eprintln!(
        "PCA model: cancellation error={error:#}, mode={:#04x}, prescale={:#04x}",
        device.file.values[MODE1], device.file.values[PRESCALE]
    );
    assert!(cancelled.get());
    device.file.assert_no_pwm_writes();
    assert_eq!(&device.file.values[0x06..0x46], original_pwm.as_slice());
    assert_eq!(device.file.values[MODE1], original_mode);
    assert_eq!(device.file.values[PRESCALE], original_prescale);
}

#[test]
fn pca_prescale_failure_restores_original_running_configuration() {
    let original_mode = 0x0b;
    let original_prescale = 0x21;
    let mut registers = Registers::new(original_mode, original_prescale);
    registers.fail_prescale_once = true;
    let mut device = registers.device();
    let error = device.initialize(&|| false).unwrap_err();
    eprintln!(
        "PCA model: prescale error={error:#}, mode={:#04x}, prescale={:#04x}",
        device.file.values[MODE1], device.file.values[PRESCALE]
    );
    assert!(format!("{error:#}").contains("injected prescale failure"));
    device.file.assert_no_pwm_writes();
    assert_eq!(device.file.values[MODE1], original_mode);
    assert_eq!(device.file.values[PRESCALE], original_prescale);
}

#[test]
fn pca_success_preserves_address_modes_and_keeps_ai_through_sleep_and_restart() {
    for original in [0x01, 0x0a, 0x2f, 0x11, 0x3a] {
        let mut device = Registers::new(original, 0x21).device();
        let snapshot = device.initialize(&|| false).unwrap();
        assert_eq!(snapshot.mode, original);
        assert_eq!(snapshot.prescale, 0x21);
        assert_eq!(device.file.values[MODE1], (original & !SLEEP) | AI);
        assert_eq!(device.file.values[PRESCALE], device.prescale());
        assert!(
            device
                .file
                .writes
                .iter()
                .filter(|b| b.len() == 2 && b[0] == 0)
                .all(|b| b[1] & AI != 0)
        );
        device.set_pwm(15, 0x0123, 0x0345).unwrap();
        assert_eq!(&device.file.values[0x42..0x46], &[0x23, 0x01, 0x45, 0x03]);
    }
}

#[test]
fn pca_cancellation_at_each_initialization_stage_restores_awake_or_sleeping_state() {
    for original in [0x0b, 0x3a, 0xba] {
        for stage in [Event::Sleep, Event::Prescale, Event::Wake, Event::Restart] {
            // An initially sleeping board without restart state has nothing to restart.
            if original == 0x3a && stage == Event::Restart {
                continue;
            }
            let mut registers = Registers::new(original, 0x21);
            registers.cancel_on = Some((stage, 1));
            let cancelled = Rc::clone(&registers.cancelled);
            let original_pwm = registers.values[0x06..0x46].to_vec();
            let mut device = registers.device();
            let error = device.initialize(&|| cancelled.get()).unwrap_err();
            assert!(format!("{error:#}").contains("cancelled"));
            device.file.assert_original(original, 0x21);
            assert_eq!(&device.file.values[0x06..0x46], original_pwm.as_slice());
            assert!(device.file.data_writes() <= 8);
            if original & SLEEP != 0 && matches!(stage, Event::Sleep | Event::Prescale) {
                assert_eq!(device.file.counts[Event::Wake as usize], 0);
            }
        }
    }
}

#[test]
fn pca_failed_or_short_initialization_writes_restore_without_suffix_retries() {
    for stage in [Event::Sleep, Event::Prescale, Event::Wake, Event::Restart] {
        for failure in [Failure::Before, Failure::After, Failure::Short(1)] {
            let mut registers = Registers::new(0x0b, 0x21);
            registers.failures.push((stage, 1, failure));
            let mut device = registers.device();
            let error = device.initialize(&|| false).unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains("restoration submitted"),
                "{stage:?}/{failure:?}: {message}"
            );
            device.file.assert_original(0x0b, 0x21);
            assert!(device.file.data_writes() <= 8);
            assert!(
                device
                    .file
                    .writes
                    .iter()
                    .all(|bytes| match bytes.as_slice() {
                        [reg] => *reg == PCA_MODE1 || *reg == PCA_PRESCALE,
                        [reg, _] => *reg == PCA_MODE1 || *reg == PCA_PRESCALE,
                        _ => false,
                    })
            );
        }
    }
}

#[test]
fn pca_snapshot_read_failure_and_external_clock_rejection_change_no_registers() {
    for stage in [Event::ReadMode, Event::ReadPrescale] {
        let mut registers = Registers::new(0x0b, 0x21);
        registers.failures.push((stage, 1, Failure::Before));
        let mut device = registers.device();
        assert!(device.initialize(&|| false).is_err());
        assert_eq!(device.file.data_writes(), 0);
        device.file.assert_original(0x0b, 0x21);
    }
    for original in [0x4b, 0x71] {
        let mut device = Registers::new(original, 0x21).device();
        let error = device.initialize(&|| false).unwrap_err();
        assert!(format!("{error:#}").contains("external clock is unsupported"));
        assert_eq!(device.file.data_writes(), 0);
        device.file.assert_original(original, 0x21);
    }
}

#[test]
fn pca_recovery_failures_keep_both_causes_and_unknown_without_new_targets() {
    for (stage, occurrence) in [
        (Event::Sleep, 2),
        (Event::Prescale, 2),
        (Event::Wake, 1),
        (Event::Restart, 1),
    ] {
        for failure in [Failure::Before, Failure::After, Failure::Short(1)] {
            let mut registers = Registers::new(0x0b, 0x21);
            registers.cancel_on = Some((Event::Prescale, 1));
            registers.failures.push((stage, occurrence, failure));
            let cancelled = Rc::clone(&registers.cancelled);
            let mut device = registers.device();
            let error = device.initialize(&|| cancelled.get()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("cancelled"), "{message}");
            assert!(message.contains("recovery:"), "{message}");
            assert!(message.contains("output state unknown"), "{message}");
            device.file.assert_no_pwm_writes();
            assert!(device.file.data_writes() <= 6);
            if stage == Event::Sleep {
                // A failed attempt to establish SLEEP never authorizes a prescale write.
                assert_eq!(device.file.counts[Event::Prescale as usize], 1);
            }
            if stage == Event::Prescale {
                // Wake is still attempted despite an uncertain frequency restoration.
                assert_eq!(device.file.values[MODE1] & SLEEP, 0);
            }
            if stage == Event::Wake {
                assert_eq!(device.file.counts[Event::ReadMode as usize], 1);
                assert_eq!(device.file.counts[Event::Restart as usize], 0);
            }
        }
    }
}

#[test]
fn pca_cancellation_after_committed_initialization_blocks_first_target_frame() {
    let mut device = Registers::new(0x01, 0x21).device();
    device.initialize(&|| false).unwrap();
    let result = submit_i2c_channels(&[90.0, 91.0], &|| true, |channel, angle| {
        device.set_servo_angle(channel, angle)
    });
    assert!(result.cancelled);
    assert!(result.error.is_none());
    assert!(result.submitted_channels.is_empty());
    assert!(result.uncertain_channels.is_empty());
    device.file.assert_no_pwm_writes();
}

#[test]
fn pca_real_pwm_short_write_retains_uncertain_channel_and_never_replays_suffix() {
    let mut device = Registers::new(0x01, 0x21).device();
    device.initialize(&|| false).unwrap();
    device
        .file
        .failures
        .push((Event::Pwm, 1, Failure::Short(2)));
    device.file.writes.clear();
    let result = submit_i2c_channels(&[90.0, 91.0], &|| false, |channel, angle| {
        device.set_servo_angle(channel, angle)
    });
    assert!(result.error.is_some());
    assert_eq!(result.uncertain_channels, vec![0]);
    assert!(result.submitted_channels.is_empty());
    assert_eq!(device.file.writes.len(), 1);
    assert_eq!(device.file.writes[0].len(), 5);
}

#[test]
fn pca_later_board_open_failure_restores_prior_boards_in_reverse_order() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut registers = [Registers::new(0x0b, 0x21), Registers::new(0x3a, 0x31)];
    for (index, model) in registers.iter_mut().enumerate() {
        model.address = 0x40 + index as u16;
        model.events = Rc::clone(&events);
    }
    let mut models = registers.iter_mut();
    let error = initialize_pca_boards(&[0x40, 0x41, 0x42], &|| false, |address| {
        if address == 0x42 {
            events.borrow_mut().clear();
            bail!("injected later open failure");
        }
        let mut device = model_device(models.next().unwrap());
        device.address = address;
        Ok(device)
    })
    .err()
    .expect("third board open must fail");
    assert!(format!("{error:#}").contains("injected later open failure"));
    registers[0].assert_original(0x0b, 0x21);
    registers[1].assert_original(0x3a, 0x31);
    let sleeps: Vec<_> = events
        .borrow()
        .iter()
        .filter(|(_, stage)| *stage == Event::Sleep)
        .map(|(address, _)| *address)
        .collect();
    assert_eq!(sleeps, [0x41, 0x40]);
}

#[test]
fn pca_later_board_init_failure_or_cancellation_restores_every_uncommitted_board() {
    for cancel in [false, true] {
        let cancelled = Rc::new(Cell::new(false));
        let mut registers = [Registers::new(0x0b, 0x21), Registers::new(0x3a, 0x31)];
        registers[1].cancelled = Rc::clone(&cancelled);
        if cancel {
            registers[1].cancel_on = Some((Event::Prescale, 1));
        } else {
            registers[1]
                .failures
                .push((Event::Prescale, 1, Failure::After));
        }
        let mut models = registers.iter_mut();
        let error = initialize_pca_boards(&[0x40, 0x41], &|| cancelled.get(), |address| {
            let mut device = model_device(models.next().unwrap());
            device.address = address;
            Ok(device)
        })
        .err()
        .expect("second board initialization must fail");
        assert!(format!("{error:#}").contains(if cancel {
            "cancelled"
        } else {
            "failure after write"
        }));
        registers[0].assert_original(0x0b, 0x21);
        registers[1].assert_original(0x3a, 0x31);
    }
}

#[test]
fn pca_prior_board_recovery_failure_does_not_abandon_remaining_board_recovery() {
    let mut registers = [Registers::new(0x0b, 0x21), Registers::new(0x0a, 0x31)];
    registers[1]
        .failures
        .push((Event::Prescale, 2, Failure::After));
    let mut models = registers.iter_mut();
    let error = initialize_pca_boards(&[0x40, 0x41, 0x42], &|| false, |address| {
        if address == 0x42 {
            bail!("injected later open failure");
        }
        let mut device = model_device(models.next().unwrap());
        device.address = address;
        Ok(device)
    })
    .err()
    .expect("construction must retain recovery failure");
    let message = format!("{error:#}");
    assert!(message.contains("output state unknown"), "{message}");
    assert!(message.contains("injected later open failure"), "{message}");
    assert!(
        message.contains("prior-board recovery: address 0x41"),
        "{message}"
    );
    assert!(message.contains("restoring PRE_SCALE"), "{message}");
    registers[0].assert_original(0x0b, 0x21);
    registers[1].assert_no_pwm_writes();
    assert_eq!(registers[1].values[MODE1] & SLEEP, 0);
}

#[test]
fn pca_cancellation_at_construction_commit_recovers_all_completed_boards() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut registers = [Registers::new(0x0b, 0x21), Registers::new(0x0a, 0x31)];
    for (index, model) in registers.iter_mut().enumerate() {
        model.address = 0x40 + index as u16;
        model.events = Rc::clone(&events);
    }
    let after_restart_checks = Cell::new(0);
    let cancelled = || {
        if events.borrow().last() != Some(&(0x41, Event::Restart)) {
            return false;
        }
        let checks = after_restart_checks.get();
        after_restart_checks.set(checks + 1);
        // Permit the board's final check; cancel at the constructor's commit check.
        checks > 0
    };
    let mut models = registers.iter_mut();
    let error = initialize_pca_boards(&[0x40, 0x41], &cancelled, |address| {
        let mut device = model_device(models.next().unwrap());
        device.address = address;
        Ok(device)
    })
    .err()
    .expect("construction commit must observe cancellation");
    assert!(format!("{error:#}").contains("cancelled"));
    assert_eq!(after_restart_checks.get(), 2);
    registers[0].assert_original(0x0b, 0x21);
    registers[1].assert_original(0x0a, 0x31);
}
