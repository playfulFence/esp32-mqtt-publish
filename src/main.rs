use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;

use std::time::Duration;

use anyhow::bail;
use log::info;
use mqtt::control::ConnectReturnCode;
use mqtt::packet::{ConnackPacket, ConnectPacket, PublishPacketRef, QoSWithPacketIdentifier};
use mqtt::{Decodable, Encodable, TopicName};

use embedded_hal::blocking::delay::DelayMs;
use embedded_svc::wifi::*;
use esp_idf_hal::delay;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::EspWifi;

static LOGGER: EspLogger = EspLogger;

// !!! SET THIS !!!
const WIFI_SSID: &str = "EspressifSystems";
const WIFI_PASS: &str = "Espressif32";

// !!! SET THIS !!!
const MQTT_ADDR: &str = ""; // host:port
const MQTT_CLIENT_ID: &str = "kirill_test";
const MQTT_TOPIC_NAME: &str = "work plssss!!!!";

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    log::set_logger(&LOGGER).map(|()| LOGGER.initialize())?;
    LOGGER.set_target_level("", log::LevelFilter::Info);

    #[allow(unused)]
    let netif_stack = Arc::new(EspNetifStack::new()?);
    #[allow(unused)]
    let sys_loop_stack = Arc::new(EspSysLoopStack::new()?);
    #[allow(unused)]
    let default_nvs = Arc::new(EspDefaultNvs::new()?);
    let _wifi = wifi(
        netif_stack.clone(),
        sys_loop_stack.clone(),
        default_nvs.clone(),
    )?;

    Ok(())
}

fn wifi(
    netif_stack: Arc<EspNetifStack>,
    sys_loop_stack: Arc<EspSysLoopStack>,
    default_nvs: Arc<EspDefaultNvs>,
) -> anyhow::Result<Box<EspWifi>> {
    let mut wifi = Box::new(EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?);

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: "EspressifSystems".into(),
        password: "Espressif32".into(),
        auth_method: AuthMethod::None,
        ..Default::default()
    }))?;

    println!("Wifi configuration set, about to get status");

    wifi.wait_status_with_timeout(Duration::from_secs(20), |status| !status.is_transitional())
        .map_err(|e| anyhow::anyhow!("Unexpected Wifi status: {:?}", e))?;

    info!("to get status");
    let status = wifi.get_status();

    info!("got status)");
    if let Status(
        ClientStatus::Started(ClientConnectionStatus::Connected(ClientIpStatus::Done(
            _ip_settings,
        ))),
        _,
    ) = status
    {
        println!("Wifi connected");
    } else {
        bail!("Unexpected Wifi status: {:?}", status);
    }

    Ok(wifi)
}




fn mqtt_connect(_: &EspWifi, mqtt_addr: &str, client_id: &str) -> anyhow::Result<TcpStream> {
    let mut stream = TcpStream::connect(mqtt_addr)?;

    let mut conn = ConnectPacket::new(client_id);
    conn.set_clean_session(true);
    let mut buf = Vec::new();
    conn.encode(&mut buf)?;
    stream.write_all(&buf[..])?;

    let conn_ack = ConnackPacket::decode(&mut stream)?;

    if conn_ack.connect_return_code() != ConnectReturnCode::ConnectionAccepted {
        bail!("MQTT failed to receive the connection accepted ack");
    }

    info!("MQTT connected");

    Ok(stream)
}

fn mqtt_publish(
    _: &EspWifi,
    stream: &mut TcpStream,
    topic_name: &str,
    message: &str,
    qos: QoSWithPacketIdentifier,
) -> anyhow::Result<()> {
    let topic = unsafe { TopicName::new_unchecked(topic_name.to_string()) };
    let bytes = message.as_bytes();

    let publish_packet = PublishPacketRef::new(&topic, qos, bytes);

    let mut buf = Vec::new();
    publish_packet.encode(&mut buf)?;
    stream.write_all(&buf[..])?;

    info!("MQTT published message {} to topic {}", message, topic_name);

    Ok(())
}
