use std::env;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;

use std::time::Duration;

use anyhow::bail;
use esp_idf_hal::prelude::Peripherals;
use log::info;


// MQTT
use mqtt::control::ConnectReturnCode;
use mqtt::packet::{ConnackPacket, ConnectPacket, PublishPacketRef, QoSWithPacketIdentifier};
use mqtt::{Decodable, Encodable, TopicName};

use embedded_hal::blocking::delay::DelayMs;

use esp_idf_hal::{delay, peripherals};
use esp_idf_svc::log::EspLogger;


use esp_idf_hal::prelude::*;
use esp_idf_hal::*;
use esp_idf_sys::*;

// Wi-Fi
use embedded_svc::wifi::*;
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::EspWifi;


// Sensors
use icm42670::{accelerometer::Accelerometer, Address, Icm42670};
use shared_bus::BusManagerSimple;
use shtcx::{shtc3, LowPower, PowerMode, ShtC3, Measurement};


static LOGGER: EspLogger = EspLogger;

// !!! SET THIS !!!
const WIFI_SSID: &str = "EspressifSystems";//env!("RUST_BOARD_MEASUREMENTS_WIFI_SSID");
const WIFI_PASS: &str = "Espressif32";//env!("RUST_BOARD_MEASUREMENTS_WIFI_PASS");

// !!! SET THIS !!!
const MQTT_ADDR: &str = "broker.hivemq.com:1883";//env!("RUST_BOARD_MEASUREMENTS_MQTT_ADDR"); // host:port
const MQTT_CLIENT_ID: &str = "esptest";//env!("RUST_BOARD_MEASUREMENTS_MQTT_CLIENT_ID");
const MQTT_TOPIC_NAME: &str = "measurements";//env!("RUST_BOARD_MEASUREMENTS_MQTT_TOPIC_NAME");

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    log::set_logger(&LOGGER).map(|()| LOGGER.initialize())?;
    LOGGER.set_target_level("", log::LevelFilter::Info);

    unsafe{


        let netif_stack = Arc::new(EspNetifStack::new()?);
        let sys_loop_stack = Arc::new(EspSysLoopStack::new()?);
        let default_nvs = Arc::new(EspDefaultNvs::new()?);


        let peripherals = Peripherals::take().unwrap();


        let i2c = peripherals.i2c0;
        let sda = peripherals.pins.gpio10;
        let scl = peripherals.pins.gpio8;

        let config = <i2c::config::MasterConfig as Default>::default().baudrate(100.kHz().into());
        let mut i2c = i2c::Master::<i2c::I2C0, _, _>::new(i2c, i2c::MasterPins { sda, scl }, config)?;
    
        let bus = BusManagerSimple::new(i2c);
        let mut icm = Icm42670::new(bus.acquire_i2c(), Address::Primary).unwrap();
        let mut sht = shtc3(bus.acquire_i2c());
        sht.start_measurement(PowerMode::LowPower);


        let _wifi = wifi(
            netif_stack.clone(),
            sys_loop_stack.clone(),
            default_nvs.clone(),
        )?;

       
        let mut mqtt_stream = mqtt_connect(&_wifi, MQTT_ADDR, MQTT_CLIENT_ID)?;
      

        loop {
        //     let mut delay = delay::FreeRtos;
            info!("Here!");
            

            let measurement = sht.get_measurement_result().unwrap();
            info!("Or here!");
            let message = format!("TEMP : {:.2}°C\tHUM : {:.3}", measurement.temperature.as_degrees_celsius(),
                                                                         measurement.humidity.as_percent());
            info!("About to send measurements : \n\t TEMP : {:.2}°C\tHUM : {:.3}", measurement.temperature.as_degrees_celsius(),
                                                                                   measurement.humidity.as_percent());
            mqtt_publish(
                &_wifi,
                &mut mqtt_stream,
                MQTT_TOPIC_NAME,
                &message,
                QoSWithPacketIdentifier::Level0,
            )?;
            
            info!("Message sent!\nAbout to start new measurement and sleep a bit");
            sht.start_measurement(PowerMode::LowPower);

            thread::sleep(Duration::from_secs(5));
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn wifi(
    netif_stack: Arc<EspNetifStack>,
    sys_loop_stack: Arc<EspSysLoopStack>,
    default_nvs: Arc<EspDefaultNvs>,
) -> anyhow::Result<Box<EspWifi>> {
    let mut wifi = Box::new(EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?);

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.into(),
        password: WIFI_PASS.into(),
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
