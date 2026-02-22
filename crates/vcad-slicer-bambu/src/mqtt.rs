//! MQTT client for Bambu printers.

use std::net::IpAddr;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::sync::{broadcast, Mutex};

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
    event_loop: Mutex<EventLoop>,
    status_tx: broadcast::Sender<PrinterStatus>,
}

impl BambuMqttClient {
    /// Connect to a Bambu printer.
    pub async fn connect(config: BambuConfig) -> Result<Self> {
        let client_id = format!("vcad_{}", uuid::Uuid::new_v4());

        // Configure MQTT options
        let mut mqtt_options = MqttOptions::new(&client_id, config.ip.to_string(), 8883);
        mqtt_options.set_credentials("bblp", &config.access_code);
        mqtt_options.set_keep_alive(Duration::from_secs(30));
        mqtt_options.set_clean_session(true);

        // Configure TLS (Bambu uses self-signed certificates)
        let tls_config = TlsConfiguration::Simple {
            ca: vec![], // Accept any certificate
            alpn: None,
            client_auth: None,
        };
        mqtt_options.set_transport(Transport::tls_with_config(tls_config));

        // Create client
        let (client, event_loop) = AsyncClient::new(mqtt_options, 100);

        // Create status broadcast channel
        let (status_tx, _) = broadcast::channel(16);

        let mqtt_client = Self {
            config,
            client,
            event_loop: Mutex::new(event_loop),
            status_tx,
        };

        // Wait for connection
        mqtt_client.wait_for_connection().await?;

        // Subscribe to topics
        mqtt_client.subscribe().await?;

        Ok(mqtt_client)
    }

    async fn wait_for_connection(&self) -> Result<()> {
        let mut event_loop = self.event_loop.lock().await;
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > self.config.timeout {
                return Err(BambuError::Timeout("connection timeout".into()));
            }

            match tokio::time::timeout(Duration::from_millis(500), event_loop.poll()).await {
                Ok(Ok(Event::Incoming(Packet::ConnAck(_)))) => {
                    return Ok(());
                }
                Ok(Ok(_)) => continue,
                Ok(Err(e)) => {
                    return Err(BambuError::ConnectionFailed(e.to_string()));
                }
                Err(_) => continue, // Timeout, keep trying
            }
        }
    }

    async fn subscribe(&self) -> Result<()> {
        // Subscribe to printer report topic
        let report_topic = format!("device/{}/report", self.config.serial);
        self.client
            .subscribe(&report_topic, QoS::AtMostOnce)
            .await
            .map_err(|e| BambuError::MqttError(e.to_string()))?;

        Ok(())
    }

    /// Send a command to the printer.
    pub async fn send_command(&self, command: PrinterCommand) -> Result<()> {
        let topic = format!("device/{}/{}", self.config.serial, command.topic_suffix());
        let payload = command.to_json().to_string();

        self.client
            .publish(&topic, QoS::AtMostOnce, false, payload)
            .await
            .map_err(|e| BambuError::MqttError(e.to_string()))?;

        Ok(())
    }

    /// Request a full status push from the printer.
    pub async fn request_status(&self) -> Result<()> {
        self.send_command(PrinterCommand::PushAll).await
    }

    /// Subscribe to status updates.
    pub fn subscribe_status(&self) -> broadcast::Receiver<PrinterStatus> {
        self.status_tx.subscribe()
    }

    /// Process incoming MQTT messages.
    /// Call this in a loop to receive status updates.
    pub async fn poll(&self) -> Result<Option<PrinterStatus>> {
        let mut event_loop = self.event_loop.lock().await;

        match tokio::time::timeout(Duration::from_millis(100), event_loop.poll()).await {
            Ok(Ok(Event::Incoming(Packet::Publish(publish)))) => {
                // Parse status from message
                if publish.topic.contains("/report") {
                    if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&publish.payload)
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
            Ok(Err(e)) => Err(BambuError::MqttError(e.to_string())),
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
