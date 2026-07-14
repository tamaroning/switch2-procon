//! BLE session worker: scan, connect, input + rumble loop.

use crate::ble_latency::LowLatencyHold;
use crate::input::{ControllerState, INPUT_CHAR_UUID};
use crate::output::{GamepadOutput, RumbleMotors, VigemStatus, create_output};
use crate::rumble::{self, RUMBLE_CHAR_UUID};
use anyhow::{Context, Result};
use btleplug::api::{
    Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures::{FutureExt, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{self, MissedTickBehavior};
use uuid::Uuid;

const SWITCH2_COMPANY_ID: u16 = 0x0553;
const NINTENDO_COMPANY_ID: u16 = 0x057e;
const SWITCH2_PRO_PID_LE: &[u8] = &[0x69, 0x20];
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    Idle,
    Scanning,
    Connecting,
    Active,
    Error,
}

impl ConnectionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Scanning => "Scanning",
            Self::Connecting => "Connecting",
            Self::Active => "Connected",
            Self::Error => "Error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub addr: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub phase: ConnectionPhase,
    pub devices: Vec<DiscoveredDevice>,
    pub selected_addr: Option<String>,
    pub live: ControllerState,
    pub vigem: VigemStatus,
    pub last_error: Option<String>,
    /// Negotiated BLE connection interval in milliseconds (Windows 11+).
    pub ble_interval_ms: Option<f32>,
    /// Input notification rate estimated over the last second.
    pub input_hz: Option<f32>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            phase: ConnectionPhase::Idle,
            devices: Vec::new(),
            selected_addr: None,
            live: ControllerState::default(),
            vigem: VigemStatus::Unsupported,
            last_error: None,
            ble_interval_ms: None,
            input_hz: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    StartScan,
    StopScan,
    Connect(String),
    Disconnect,
    Quit,
}

/// Handle to the background BLE session.
pub struct SessionHandle {
    pub state: Arc<Mutex<AppState>>,
    pub cmd_tx: mpsc::UnboundedSender<Command>,
}

impl SessionHandle {
    /// Spawn a tokio worker on a dedicated thread.
    pub fn spawn() -> Self {
        let state = Arc::new(Mutex::new(AppState::default()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let state_worker = state.clone();
        let cmd_tx_worker = cmd_tx.clone();
        std::thread::Builder::new()
            .name("switch2-session".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(session_worker(state_worker, cmd_tx_worker, cmd_rx));
            })
            .expect("spawn session thread");
        Self { state, cmd_tx }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }
}

fn payload_has_pid(payload: &[u8]) -> bool {
    payload
        .windows(SWITCH2_PRO_PID_LE.len())
        .any(|w| w == SWITCH2_PRO_PID_LE)
}

fn is_switch2_pro_mfg(manufacturer_data: &HashMap<u16, Vec<u8>>) -> bool {
    for id in [SWITCH2_COMPANY_ID, NINTENDO_COMPANY_ID] {
        if manufacturer_data
            .get(&id)
            .is_some_and(|payload| payload_has_pid(payload))
        {
            return true;
        }
    }
    manufacturer_data
        .values()
        .any(|payload| payload.windows(4).any(|w| w == [0x7e, 0x05, 0x69, 0x20]))
}

fn significant_change(prev: &ControllerState, next: &ControllerState) -> bool {
    if prev.buttons != next.buttons {
        return true;
    }
    (next.left.x - prev.left.x).abs() > 0.05
        || (next.left.y - prev.left.y).abs() > 0.05
        || (next.right.x - prev.right.x).abs() > 0.05
        || (next.right.y - prev.right.y).abs() > 0.05
}

async fn peripheral_summary(
    central: &Adapter,
    id: &PeripheralId,
) -> Option<(Peripheral, String, String, HashMap<u16, Vec<u8>>)> {
    let p = central.peripheral(id).await.ok()?;
    let props = p.properties().await.ok().flatten();
    let name = props
        .as_ref()
        .and_then(|pr| pr.local_name.clone())
        .unwrap_or_else(|| "(no name)".into());
    let addr = p.address().to_string().to_lowercase();
    let mfg = props.map(|pr| pr.manufacturer_data).unwrap_or_default();
    Some((p, addr, name, mfg))
}

fn set_phase(state: &Arc<Mutex<AppState>>, phase: ConnectionPhase) {
    if let Ok(mut s) = state.lock() {
        s.phase = phase;
    }
}

fn set_error(state: &Arc<Mutex<AppState>>, msg: impl Into<String>) {
    if let Ok(mut s) = state.lock() {
        s.phase = ConnectionPhase::Error;
        s.last_error = Some(msg.into());
    }
}

fn upsert_device(state: &Arc<Mutex<AppState>>, addr: String, name: String) {
    if let Ok(mut s) = state.lock() {
        if let Some(d) = s.devices.iter_mut().find(|d| d.addr == addr) {
            if name != "(no name)" {
                d.name = name;
            }
        } else {
            s.devices.push(DiscoveredDevice { addr, name });
        }
    }
}

async fn write_rumble(
    controller: &Peripheral,
    rumble_char: &Characteristic,
    seq: &mut u8,
    motors: RumbleMotors,
) -> Result<()> {
    let pkt = rumble::build_packet(*seq, motors.0, motors.1);
    *seq = seq.wrapping_add(1) & 0x0F;
    controller
        .write(rumble_char, &pkt, WriteType::WithoutResponse)
        .await
        .context("HD rumble write failed")?;
    Ok(())
}

/// Rumble BLE writes run off the input path. On Windows, GATT writes await the
/// radio queue; doing them in the same select as notifications stalls the
/// consumer and overflows btleplug's 16-slot broadcast buffer (drops = lag).
async fn rumble_loop(
    controller: Peripheral,
    rumble_char: Characteristic,
    mut rumble_rx: watch::Receiver<RumbleMotors>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) {
    let mut seq = 0u8;
    let mut last_motors: RumbleMotors = (0, 0);
    let mut rumble_tick = time::interval(Duration::from_millis(16));
    rumble_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            result = rumble_rx.changed() => {
                if result.is_err() {
                    break;
                }
                let motors = *rumble_rx.borrow_and_update();
                if write_rumble(&controller, &rumble_char, &mut seq, motors)
                    .await
                    .is_ok()
                {
                    last_motors = motors;
                }
            }
            _ = rumble_tick.tick() => {
                let motors = *rumble_rx.borrow();
                if motors == (0, 0) {
                    continue;
                }
                if write_rumble(&controller, &rumble_char, &mut seq, motors)
                    .await
                    .is_ok()
                {
                    last_motors = motors;
                }
            }
        }
    }

    if last_motors != (0, 0) {
        let _ = write_rumble(&controller, &rumble_char, &mut seq, (0, 0)).await;
    }
}

async fn run_input_loop(
    controller: &Peripheral,
    filter_uuid: Option<Uuid>,
    rumble_char: Option<Characteristic>,
    output: &mut dyn GamepadOutput,
    rumble_rx: watch::Receiver<RumbleMotors>,
    state: Arc<Mutex<AppState>>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let rumble_task = rumble_char.map(|c| {
        let controller = controller.clone();
        let rumble_rx = rumble_rx.clone();
        let cancel = cancel.clone();
        tokio::spawn(rumble_loop(controller, c, rumble_rx, cancel))
    });

    let mut last = ControllerState::default();
    let mut notifications = controller.notifications().await?;
    let mut rate_window = Instant::now();
    let mut rate_count: u32 = 0;
    let mut params_tick = time::interval(Duration::from_secs(2));
    params_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Skip the immediate first tick so we don't race the low-latency request.
    params_tick.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    break;
                }
            }
            _ = params_tick.tick() => {
                if let Ok(Some(params)) = controller.connection_parameters().await {
                    let ms = params.interval_us as f32 / 1000.0;
                    if let Ok(mut s) = state.lock() {
                        s.ble_interval_ms = Some(ms);
                    }
                }
            }
            data = notifications.next() => {
                let Some(first) = data else { break; };
                // Drain ready reports; keep the newest matching packet.
                let mut newest = None;
                let mut candidate = first;
                let mut ended = false;
                loop {
                    if filter_uuid.is_none_or(|u| candidate.uuid == u) {
                        newest = Some(candidate);
                    }
                    match notifications.next().now_or_never() {
                        Some(Some(more)) => candidate = more,
                        Some(None) => {
                            ended = true;
                            break;
                        }
                        None => break,
                    }
                }
                if let Some(data) = newest {
                    let Some(parsed) = ControllerState::parse(&data.value) else {
                        if ended {
                            break;
                        }
                        continue;
                    };
                    rate_count = rate_count.saturating_add(1);
                    let elapsed = rate_window.elapsed();
                    if elapsed >= Duration::from_secs(1) {
                        let hz = rate_count as f32 / elapsed.as_secs_f32();
                        rate_count = 0;
                        rate_window = Instant::now();
                        if let Ok(mut s) = state.lock() {
                            s.input_hz = Some(hz);
                        }
                    }
                    if significant_change(&last, &parsed) {
                        last = parsed;
                        if let Ok(mut s) = state.lock() {
                            s.live = parsed;
                        }
                    }
                    output.update(&parsed)?;
                }
                if ended {
                    break;
                }
            }
        }
    }

    if let Some(task) = rumble_task {
        task.abort();
    }
    Ok(())
}

async fn connect_and_run(
    central: &Adapter,
    addr: &str,
    known: &HashMap<String, PeripheralId>,
    state: Arc<Mutex<AppState>>,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    set_phase(&state, ConnectionPhase::Connecting);
    if let Ok(mut s) = state.lock() {
        s.selected_addr = Some(addr.to_string());
        s.last_error = None;
    }

    let peripheral = if let Some(id) = known.get(addr) {
        central.peripheral(id).await.context("peripheral by id")?
    } else {
        // Brief scan for the address.
        let mut events = central.events().await?;
        central.start_scan(ScanFilter::default()).await?;
        let found = time::timeout(Duration::from_secs(15), async {
            while let Some(event) = events.next().await {
                match event {
                    CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                        if let Some((p, a, _, _)) = peripheral_summary(central, &id).await
                            && a == addr
                        {
                            return Ok::<Peripheral, anyhow::Error>(p);
                        }
                    }
                    _ => {}
                }
            }
            anyhow::bail!("Event stream ended while looking for {addr}")
        })
        .await
        .context("Timed out looking for device")??;
        let _ = central.stop_scan().await;
        found
    };

    peripheral.connect().await.context("BLE connect failed")?;
    peripheral
        .discover_services()
        .await
        .context("discover_services failed")?;

    // Prefer a short connection interval. Hold the WinRT request for the session
    // so Windows does not revert to Balanced as soon as the handle is dropped.
    let addr_u64 = peripheral.address().into();
    let (low_latency_hold, initial_interval_ms) = LowLatencyHold::acquire(addr_u64).await;
    if let Ok(mut s) = state.lock() {
        s.ble_interval_ms = initial_interval_ms;
        s.input_hz = None;
    }

    let input_uuid = Uuid::parse_str(INPUT_CHAR_UUID)?;
    let rumble_uuid = Uuid::parse_str(RUMBLE_CHAR_UUID)?;
    let mut input_char: Option<Characteristic> = None;
    let mut rumble_char: Option<Characteristic> = None;
    let mut notify_chars = Vec::new();
    for service in peripheral.services() {
        for c in &service.characteristics {
            if c.uuid == input_uuid {
                input_char = Some(c.clone());
            }
            if c.uuid == rumble_uuid {
                rumble_char = Some(c.clone());
            }
            if c.properties
                .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
            {
                notify_chars.push(c.clone());
            }
        }
    }

    let filter_uuid = if let Some(c) = input_char {
        peripheral.subscribe(&c).await?;
        Some(input_uuid)
    } else if !notify_chars.is_empty() {
        for c in &notify_chars {
            let _ = peripheral.subscribe(c).await;
        }
        None
    } else {
        anyhow::bail!("No input characteristic found");
    };

    let (bundle, vigem) = create_output();
    if let Ok(mut s) = state.lock() {
        s.vigem = vigem;
        s.phase = ConnectionPhase::Active;
        s.last_error = None;
    }

    let OutputBundleParts {
        mut gamepad,
        rumble_rx,
    } = OutputBundleParts::from(bundle);

    let result = run_input_loop(
        &peripheral,
        filter_uuid,
        rumble_char,
        gamepad.as_mut(),
        rumble_rx,
        state.clone(),
        cancel,
    )
    .await;

    drop(low_latency_hold);
    let _ = peripheral.disconnect().await;
    if let Ok(mut s) = state.lock() {
        s.live = ControllerState::default();
        s.ble_interval_ms = None;
        s.input_hz = None;
        if s.phase == ConnectionPhase::Active {
            s.phase = ConnectionPhase::Idle;
        }
    }
    result
}

struct OutputBundleParts {
    gamepad: Box<dyn GamepadOutput>,
    rumble_rx: watch::Receiver<RumbleMotors>,
}

impl From<crate::output::OutputBundle> for OutputBundleParts {
    fn from(b: crate::output::OutputBundle) -> Self {
        Self {
            gamepad: b.gamepad,
            rumble_rx: b.rumble_rx,
        }
    }
}

async fn scan_loop(
    central: Adapter,
    state: Arc<Mutex<AppState>>,
    known: Arc<Mutex<HashMap<String, PeripheralId>>>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    cmd_tx: mpsc::UnboundedSender<Command>,
) {
    set_phase(&state, ConnectionPhase::Scanning);
    if let Ok(mut s) = state.lock() {
        s.devices.clear();
        s.last_error = None;
    }

    let Ok(mut events) = central.events().await else {
        set_error(&state, "Failed to open BLE event stream");
        return;
    };
    if central.start_scan(ScanFilter::default()).await.is_err() {
        set_error(&state, "Failed to start BLE scan");
        return;
    }

    let mut timed_out = false;
    let deadline = time::sleep(SCAN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = cancel.changed() => {
                // Sender dropped or cancel requested — leave the loop.
                if result.is_err() || *cancel.borrow() {
                    break;
                }
            }
            _ = &mut deadline => {
                timed_out = true;
                break;
            }
            event = events.next() => {
                let Some(event) = event else { break; };
                // Match using advertisement payload only — never await before the
                // filter. btleplug's event broadcast buffer is tiny (16); awaiting
                // properties for every nearby BLE device causes Lagged drops and
                // the Switch 2 mfg packet is lost.
                let CentralEvent::ManufacturerDataAdvertisement {
                    id,
                    manufacturer_data,
                } = event
                else {
                    continue;
                };
                if !is_switch2_pro_mfg(&manufacturer_data) {
                    continue;
                }
                // WinRT emits ManufacturerDataAdvertisement inside update_properties
                // before add_peripheral; retry briefly if the peripheral is not listed yet.
                let mut summary = None;
                for _ in 0..20 {
                    summary = peripheral_summary(&central, &id).await;
                    if summary.is_some() {
                        break;
                    }
                    time::sleep(Duration::from_millis(5)).await;
                }
                let Some((_p, addr, name, _)) = summary else {
                    continue;
                };
                if let Ok(mut k) = known.lock() {
                    k.insert(addr.clone(), id);
                }
                upsert_device(&state, addr.clone(), name);
                if let Ok(mut s) = state.lock() {
                    s.selected_addr = Some(addr.clone());
                }
                // First match → auto-connect (same as the old CLI flow).
                let _ = cmd_tx.send(Command::Connect(addr));
                break;
            }
        }
    }

    let _ = central.stop_scan().await;
    if let Ok(mut s) = state.lock()
        && s.phase == ConnectionPhase::Scanning
    {
        s.phase = ConnectionPhase::Idle;
        if timed_out {
            s.last_error = Some(
                "No controller found within 30s. Put it in pairing mode and press Rescan.".into(),
            );
        }
    }
}

async fn session_worker(
    state: Arc<Mutex<AppState>>,
    cmd_tx: mpsc::UnboundedSender<Command>,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
) {
    // Probe ViGEm once for status (and drop the temporary pad).
    {
        let (_bundle, vigem) = create_output();
        if let Ok(mut s) = state.lock() {
            s.vigem = vigem;
        }
        // bundle dropped here — unplugs temporary pad
    }

    let manager = match Manager::new().await {
        Ok(m) => m,
        Err(e) => {
            set_error(&state, format!("Bluetooth manager failed: {e}"));
            while let Some(cmd) = cmd_rx.recv().await {
                if matches!(cmd, Command::Quit) {
                    break;
                }
            }
            return;
        }
    };
    let central = match manager.adapters().await {
        Ok(adapters) => adapters.into_iter().next(),
        Err(e) => {
            set_error(&state, format!("No Bluetooth adapter: {e}"));
            None
        }
    };
    let Some(central) = central else {
        set_error(&state, "No Bluetooth adapter found");
        while let Some(cmd) = cmd_rx.recv().await {
            if matches!(cmd, Command::Quit) {
                break;
            }
        }
        return;
    };

    let known: Arc<Mutex<HashMap<String, PeripheralId>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut scan_cancel_tx: Option<watch::Sender<bool>> = None;
    let mut scan_task: Option<JoinHandle<()>> = None;
    let mut conn_cancel_tx: Option<watch::Sender<bool>> = None;
    let mut conn_task: Option<JoinHandle<()>> = None;

    let stop_scan = |scan_cancel_tx: &mut Option<watch::Sender<bool>>,
                     scan_task: &mut Option<JoinHandle<()>>| {
        if let Some(tx) = scan_cancel_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(h) = scan_task.take() {
            h.abort();
        }
    };

    let stop_conn = |conn_cancel_tx: &mut Option<watch::Sender<bool>>,
                     conn_task: &mut Option<JoinHandle<()>>| {
        if let Some(tx) = conn_cancel_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(h) = conn_task.take() {
            h.abort();
        }
    };

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::Quit => {
                stop_scan(&mut scan_cancel_tx, &mut scan_task);
                stop_conn(&mut conn_cancel_tx, &mut conn_task);
                break;
            }
            Command::StartScan => {
                stop_scan(&mut scan_cancel_tx, &mut scan_task);
                let (tx, rx) = watch::channel(false);
                scan_cancel_tx = Some(tx);
                let central = central.clone();
                let state = state.clone();
                let known = known.clone();
                let cmd_tx = cmd_tx.clone();
                scan_task = Some(tokio::spawn(async move {
                    scan_loop(central, state, known, rx, cmd_tx).await;
                }));
            }
            Command::StopScan => {
                stop_scan(&mut scan_cancel_tx, &mut scan_task);
                set_phase(&state, ConnectionPhase::Idle);
            }
            Command::Disconnect => {
                stop_conn(&mut conn_cancel_tx, &mut conn_task);
                set_phase(&state, ConnectionPhase::Idle);
                if let Ok(mut s) = state.lock() {
                    s.live = ControllerState::default();
                    s.ble_interval_ms = None;
                    s.input_hz = None;
                }
                // Resume scanning so the next advertisement auto-connects again.
                let _ = cmd_tx.send(Command::StartScan);
            }
            Command::Connect(addr) => {
                stop_scan(&mut scan_cancel_tx, &mut scan_task);
                stop_conn(&mut conn_cancel_tx, &mut conn_task);
                let _ = central.stop_scan().await;
                let (tx, rx) = watch::channel(false);
                conn_cancel_tx = Some(tx);
                let central = central.clone();
                let state = state.clone();
                let cmd_tx = cmd_tx.clone();
                let known_map = known.lock().map(|k| k.clone()).unwrap_or_default();
                let addr_l = addr.to_lowercase();
                conn_task = Some(tokio::spawn(async move {
                    if let Err(e) =
                        connect_and_run(&central, &addr_l, &known_map, state.clone(), rx).await
                    {
                        set_error(&state, e.to_string());
                    }
                    // Dropped link or cancel → scan again and auto-connect on next sighting.
                    let _ = cmd_tx.send(Command::StartScan);
                }));
            }
        }
    }
}
