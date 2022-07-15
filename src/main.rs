use std::env;
use std::f32::consts::E;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;

use std::time::Duration;

use anyhow::bail;
use esp_idf_hal::prelude::Peripherals;
use log::info;



use embedded_hal::blocking::delay::DelayMs;

use esp_idf_hal::{delay, peripherals};


// MQTT stuff
use esp_idf_svc::{
    log::EspLogger,
    mqtt::client::*,
};
use embedded_svc::mqtt::client::{Client, Connection, MessageImpl, Publish, QoS};


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
const MQTT_CLIENT_ID: &str = "playfulFence";//env!("RUST_BOARD_MEASUREMENTS_MQTT_CLIENT_ID");
const MQTT_TOPIC_NAME: &str = "esp-clock/measurements";//env!("RUST_BOARD_MEASUREMENTS_MQTT_TOPIC_NAME");

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

       
        let mqtt_config = MqttClientConfiguration {
            client_id: Some("esp-clock/measurements"),
            ..Default::default()
        };
        
        let broker_url = "mqtt://broker.hivemq.com:1883";

        info!("About to connect mqtt-client");
        let (mut client, mut connection) = 
            EspMqttClient::new_with_conn(broker_url, &mqtt_config)?;

        info!("Connected");

        // Need to immediately start pumping the connection for messages, or else subscribe() and publish() below will not work
        // Note that when using the alternative constructor - `EspMqttClient::new` - you don't need to
        // spawn a new thread, as the messages will be pumped with a backpressure into the callback you provide.
        // Yet, you still need to efficiently process each message in the callback without blocking for too long.
        //
        // Note also that if you go to http://tools.emqx.io/ and then connect and send a message to topic
        // "rust-esp32-std-demo", the client configured here should receive it.


        thread::spawn(move || {
            info!("MQTT Listening for messages");
    
            while let Some(msg) = connection.next() {
                match msg {
                    Err(e) => info!("MQTT Message ERROR: {}", e),
                    Ok(msg) => info!("MQTT Message: {:?}", msg),
                }
            }
    
            info!("MQTT connection loop exit");
        });

        client.subscribe("esp-clock/measurements", QoS::AtMostOnce);

        info!("Subscribed to \"measurements\" topic!");

        client.publish("esp-clock/measurements", QoS::AtMostOnce, false, "Is someone there?".as_bytes())?;

        info!("Published a message to topic");
      

        loop {
    
            let measurement = sht.get_measurement_result().unwrap();
            let message = format!("TEMP : {:.2}°C\tHUM : {:.3}", measurement.temperature.as_degrees_celsius(),
                                                                         measurement.humidity.as_percent());
            info!("About to send measurements : {}", message);
            
            client.publish("measurements", QoS::AtMostOnce, false, message.as_bytes())?;
            info!("Message sent!\n\tAbout to start new measurement and sleep a bit");

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
