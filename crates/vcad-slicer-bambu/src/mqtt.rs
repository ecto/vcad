//! MQTT client for Bambu printers.

use std::net::IpAddr;
use std::time::Duration;

use rumqttc::{
    AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, TlsConfiguration, Transport,
};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::commands::PrinterCommand;
use crate::error::{BambuError, Result};
use crate::status::PrinterStatus;

/// Bambu printer connection configuration.
#[derive(Debug, Clone)]
pub struct BambuConfig {
    /// Printer IP address.
    pub ip: IpAddr,
    /// Printer serial number.
    pub serial: String,
    /// Access code (from printer's LAN mode settings).
    pub access_code: String,
    /// Connection timeout.
    pub timeout: Duration,
}

impl BambuConfig {
    /// Create a new configuration.
    pub fn new(ip: IpAddr, serial: String, access_code: String) -> Self {
        Self {
            ip,
            serial,
            access_code,
            timeout: Duration::from_secs(10),
        }
    }
}

/// MQTT client for Bambu printer communication.
pub struct BambuMqttClient {
    config: BambuConfig,
    client: AsyncClient,
    incoming: Mutex<mpsc::UnboundedReceiver<Result<Event>>>,
    status_tx: broadcast::Sender<PrinterStatus>,
}

impl BambuMqttClient {
    /// Connect to a Bambu printer.
    pub async fn connect(config: BambuConfig) -> Result<Self> {
        let client_id = format!("vcad_{}", uuid::Uuid::new_v4());

        debug_step(format!("configuring TCP destination {}:8883", config.ip));

        // Configure MQTT options. Bambu's local broker uses MQTT 3.1.1 and
        // rumqttc 0.24 emits that version from MqttOptions by default.
        let mut mqtt_options = MqttOptions::new(&client_id, config.ip.to_string(), 8883);
        mqtt_options.set_credentials("bblp", &config.access_code);
        mqtt_options.set_keep_alive(Duration::from_secs(30));
        mqtt_options.set_clean_session(true);

        // Bambu's per-device CA isn't a public root. `Simple { ca: [] }`
        // builds an EMPTY root store, which rejects the printer's cert every
        // time; hand rumqttc a config that accepts it instead.
        let tls_config = TlsConfiguration::Rustls(crate::tls::mqtt_client_config());
        mqtt_options.set_transport(Transport::tls_with_config(tls_config));
        debug_step("TLS configured with LAN certificate verification disabled".to_string());

        // Create client
        let (client, event_loop) = AsyncClient::new(mqtt_options, 100);
        debug_step(format!(
            "MQTT 3.1.1 client {} configured (keepalive 30s, username bblp)",
            client_id
        ));

        // Create status broadcast channel
        let (status_tx, _) = broadcast::channel(16);
        let (event_tx, incoming) = mpsc::unbounded_channel();
        tokio::spawn(run_event_loop(event_loop, event_tx));

        let mqtt_client = Self {
            config,
            client,
            incoming: Mutex::new(incoming),
            status_tx,
        };

        // Wait for connection
        debug_step("waiting for MQTT 3.1.1 CONNACK".to_string());
        mqtt_client.wait_for_connection().await?;

        // Subscribe to topics
        debug_step(format!(
            "subscribing to device/{}/report",
            mqtt_client.config.serial
        ));
        mqtt_client.subscribe().await?;

        Ok(mqtt_client)
    }

    async fn wait_for_connection(&self) -> Result<()> {
        tokio::time::timeout(self.config.timeout, async {
            loop {
                match self.next_event().await? {
                    Event::Incoming(Packet::ConnAck(_)) => {
                        debug_step("MQTT CONNACK received".to_string());
                        return Ok(());
                    }
                    _ => continue,
                }
            }
        })
        .await
        .map_err(|_| BambuError::Timeout("connection timeout".into()))?
    }

    async fn next_event(&self) -> Result<Event> {
        self.incoming.lock().await.recv().await.unwrap_or_else(|| {
            Err(BambuError::ConnectionFailed(
                "MQTT event loop stopped".into(),
            ))
        })
    }

    async fn subscribe(&self) -> Result<()> {
        // Subscribe to printer report topic
        let report_topic = format!("device/{}/report", self.config.serial);
        self.client
            .subscribe(&report_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| BambuError::MqttError(e.to_string()))?;
        debug_step("report subscription queued".to_string());

        Ok(())
    }

    /// Send a command to the printer.
    pub async fn send_command(&self, command: PrinterCommand) -> Result<()> {
        let topic = format!("device/{}/{}", self.config.serial, command.topic_suffix());
        let payload = command.to_json().to_string();
        debug_step(format!("publishing {} command", command.topic_suffix()));

        self.client
            .publish(&topic, QoS::AtMostOnce, false, payload)
            .await
            .map_err(|e| BambuError::MqttError(e.to_string()))?;

        Ok(())
    }

    /// Request a full status push from the printer.
    pub async fn request_status(&self) -> Result<()> {
        debug_step(format!(
            "requesting status on device/{}/request (pushall)",
            self.config.serial
        ));
        self.send_command(PrinterCommand::PushAll).await
    }

    /// Subscribe to status updates.
    pub fn subscribe_status(&self) -> broadcast::Receiver<PrinterStatus> {
        self.status_tx.subscribe()
    }

    /// Process incoming MQTT messages.
    /// Call this in a loop to receive status updates.
    pub async fn poll(&self) -> Result<Option<PrinterStatus>> {
        match tokio::time::timeout(Duration::from_millis(100), self.next_event()).await {
            Ok(Ok(Event::Incoming(Packet::Publish(publish)))) => {
                // Parse status from message
                if publish.topic.contains("/report") {
                    if let Ok(payload) =
                        serde_json::from_slice::<serde_json::Value>(&publish.payload)
                    {
                        let status = PrinterStatus::from_mqtt_payload(&payload);
                        // Broadcast to subscribers
                        let _ = self.status_tx.send(status.clone());
                        return Ok(Some(status));
                    }
                }
                Ok(None)
            }
            Ok(Ok(_)) => Ok(None),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(None), // Timeout
        }
    }

    /// Get the printer serial number.
    pub fn serial(&self) -> &str {
        &self.config.serial
    }

    /// Get the printer IP address.
    pub fn ip(&self) -> IpAddr {
        self.config.ip
    }
}

async fn run_event_loop(mut event_loop: EventLoop, tx: mpsc::UnboundedSender<Result<Event>>) {
    loop {
        let event = event_loop
            .poll()
            .await
            .map_err(|error| BambuError::ConnectionFailed(error.to_string()));
        let failed = event.is_err();
        if tx.send(event).is_err() || failed {
            break;
        }
    }
}

fn debug_step(message: String) {
    if printer_debug_enabled() {
        eprintln!("[vcad-printer] {message}");
    }
}

fn printer_debug_enabled() -> bool {
    debug_flag_is_enabled(std::env::var("VCAD_PRINTER_DEBUG").ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::{debug_flag_is_enabled, run_event_loop};
    use rumqttc::{Event, MqttOptions, Packet};
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[test]
    fn printer_debug_flag_is_opt_in() {
        assert!(!debug_flag_is_enabled(None));
        assert!(!debug_flag_is_enabled(Some("0")));
    }

    #[test]
    fn printer_debug_flag_requires_exact_one() {
        assert!(debug_flag_is_enabled(Some("1")));
        assert!(!debug_flag_is_enabled(Some("true")));
        assert!(!debug_flag_is_enabled(None));
    }

    #[tokio::test]
    async fn event_loop_driver_forwards_connection_errors() {
        let options = MqttOptions::new("poll_test", "127.0.0.1", 9);
        let (_, event_loop) = rumqttc::AsyncClient::new(options, 10);
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(run_event_loop(event_loop, tx));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("event loop should be polled")
            .expect("event loop should report its result");
        assert!(matches!(
            event,
            Err(crate::error::BambuError::ConnectionFailed(_))
        ));
    }
}

fn debug_flag_is_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

/// High-level printer interface.
pub struct BambuPrinter {
    client: BambuMqttClient,
    last_status: Mutex<Option<PrinterStatus>>,
}

impl BambuPrinter {
    /// Connect to a printer.
    pub async fn connect(ip: IpAddr, serial: &str, access_code: &str) -> Result<Self> {
        let config = BambuConfig::new(ip, serial.to_string(), access_code.to_string());
        let client = BambuMqttClient::connect(config).await?;

        Ok(Self {
            client,
            last_status: Mutex::new(None),
        })
    }

    /// Get current printer status.
    pub async fn status(&self) -> Result<PrinterStatus> {
        // Request fresh status
        self.client.request_status().await?;

        // Wait for response (with timeout)
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(status) = self.client.poll().await? {
                *self.last_status.lock().await = Some(status.clone());
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Return cached status if available
        self.last_status
            .lock()
            .await
            .clone()
            .ok_or_else(|| BambuError::Timeout("status request timed out".into()))
    }

    /// Pause the current print.
    pub async fn pause(&self) -> Result<()> {
        self.client.send_command(PrinterCommand::PrintPause).await
    }

    /// Resume a paused print.
    pub async fn resume(&self) -> Result<()> {
        self.client.send_command(PrinterCommand::PrintResume).await
    }

    /// Stop the current print.
    pub async fn stop(&self) -> Result<()> {
        self.client.send_command(PrinterCommand::PrintStop).await
    }

    /// Set print speed level (1-4).
    pub async fn set_speed(&self, level: u8) -> Result<()> {
        let level = level.clamp(1, 4);
        self.client
            .send_command(PrinterCommand::SetSpeed(level))
            .await
    }

    /// Set nozzle temperature.
    pub async fn set_nozzle_temp(&self, temp: u32) -> Result<()> {
        self.client
            .send_command(PrinterCommand::SetNozzleTemp(temp))
            .await
    }

    /// Set bed temperature.
    pub async fn set_bed_temp(&self, temp: u32) -> Result<()> {
        self.client
            .send_command(PrinterCommand::SetBedTemp(temp))
            .await
    }

    /// Send raw G-code line.
    pub async fn send_gcode(&self, gcode: &str) -> Result<()> {
        self.client
            .send_command(PrinterCommand::GcodeLine(gcode.to_string()))
            .await
    }

    /// Turn chamber light on/off.
    pub async fn set_light(&self, on: bool) -> Result<()> {
        self.client
            .send_command(PrinterCommand::SetLed {
                node: "chamber_light".into(),
                mode: if on { "on" } else { "off" }.into(),
            })
            .await
    }

    /// Upload a 3MF file and start printing.
    ///
    /// Uploads via FTPS, then sends a PrintStart MQTT command.
    pub async fn print_3mf(&self, filename: &str, data: &[u8]) -> Result<()> {
        // Upload via FTPS
        crate::ftp::upload_3mf(
            self.client.ip(),
            &self.client.config.access_code,
            filename,
            data,
        )
        .await?;

        // Send print start command
        self.client
            .send_command(PrinterCommand::PrintStart {
                file: format!("/sdcard/{}", filename),
                plate_index: 0,
                use_ams: false,
                ams_mapping: vec![0],
                bed_leveling: true,
                flow_calibration: true,
                vibration_calibration: true,
                skip_objects: vec![],
            })
            .await?;

        Ok(())
    }

    /// Get the underlying MQTT client.
    pub fn mqtt_client(&self) -> &BambuMqttClient {
        &self.client
    }
}
