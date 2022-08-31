use std::time::Duration;
use anyhow::bail;
use log::info;

// Common IDF stuff 
use esp_idf_hal::prelude::*;
use esp_idf_hal::*;
use esp_idf_sys::*;

// Peripheral stuff
use esp_idf_hal::{prelude::Peripherals, spi::MasterBus};

// Multithreading
use std::sync::mpsc::channel;
use std::thread;

// MQTT stuff
use esp_idf_svc::{
    log::EspLogger,
    mqtt::client::*,
};
use embedded_svc::mqtt::client::{Client, Connection, MessageImpl, Publish, QoS, Event::*, Message};
use std::str;

// Wi-Fi
use embedded_svc::wifi::*;
use esp_idf_svc::netif::EspNetifStack;
use esp_idf_svc::nvs::EspDefaultNvs;
use esp_idf_svc::sysloop::EspSysLoopStack;
use esp_idf_svc::wifi::EspWifi;
use std::sync::Arc;


// Sensors
use icm42670::{accelerometer::Accelerometer, Address, Icm42670};
use shared_bus::BusManagerSimple;
use shtcx::{shtc3, LowPower, PowerMode, ShtC3, Measurement};


#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    mqtt_user: &'static str,
    #[default("")]
    broker_url: &'static str,
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_pass: &'static str,
    #[default("measurements")]
    topic_name: &'static str,
}

fn main() -> anyhow::Result<()> {
    esp_idf_sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let app_config = CONFIG; 

    /* Setup some stuff for WiFi initialization */
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
    //let mut icm = Icm42670::new(bus.acquire_i2c(), Address::Primary).unwrap(); // in case you want to collect gyroscope and accelerometer data
    let mut sht = shtc3(bus.acquire_i2c());
    
    /* If your code is panicking here, consider using LowPower mode since NormalMode may cause 
        code panicking if you're trying to take measurements "too early" */
    sht.start_measurement(PowerMode::NormalMode);


    info!("About to initialize WiFi (SSID: {}, PASS: {})", app_config.wifi_ssid, app_config.wifi_pass);    
    let _wifi = wifi(
            netif_stack.clone(),
            sys_loop_stack.clone(),
            default_nvs.clone(),
            app_config.wifi_ssid,
            app_config.wifi_pass,
        )?;

    
    let mqtt_config = MqttClientConfiguration {
        client_id: Some(app_config.mqtt_user),
        ..Default::default()
    };


    info!("About to connect mqtt-client");
    let (mut client, mut connection) = 
        EspMqttClient::new_with_conn(app_config.broker_url, &mqtt_config)?;

    info!("Connected");

    // Need to immediately start pumping the connection for messages, or else subscribe() and publish() below will not work
    // Note that when using the alternative constructor - `EspMqttClient::new` - you don't need to
    // spawn a new thread, as the messages will be pumped with a backpressure into the callback you provide.
    // Yet, you still need to efficiently process each message in the callback without blocking for too long.

    thread::spawn(move || {
        info!("MQTT Listening for messages");
    
        while let Some(msg) = connection.next() {
            match msg {
                Err(e) => info!("MQTT Message ERROR: {}", e),
                Ok(message) => {
                    match message {
                        Received(recieved_bytes) => {
                            match str::from_utf8(recieved_bytes.data()) {
                                Err(e) => info!("MQTT Error : unreadable message! ({})", e),
                                Ok(measurements) => info!("MQTT Message : {}", measurements)
                            }
                        },
                        BeforeConnect => info!("MQTT Message : Before connect"),
                        Connected(tf) => info!("MQTT Message : Connected({})", tf),
                        Disconnected => info!("MQTT Message : Disconnected"),
                        Subscribed(message_id) => info!("MQTT Message : Subscribed({})", message_id),
                        Unsubscribed(message_id) => info!("MQTT Message : Unsubscribed({})", message_id),
                        Published(message_id) => info!("MQTT Message : Published({})", message_id),
                        Deleted(message_id) => info!("MQTT Message : Deleted({})", message_id),
                    } 
                },
            }
        }
        info!("MQTT connection loop exit");
    });

    loop {
        let measurement = sht.get_measurement_result().unwrap();
        let message = format!("{:.0}°C {:.0}%RH", measurement.temperature.as_degrees_celsius(),
                                                            measurement.humidity.as_percent());
        info!("About to send measurements : {}", message);
        
        client.publish(app_config.topic_name, QoS::AtLeastOnce, false, message.as_bytes())?;
        info!("Message sent!\n\tAbout to start new measurement and sleep a bit");

        /* If your code is panicking here, consider using LowPower mode since NormalMode may cause 
            code panicking if you're trying to take measurements "too early" */
        sht.start_measurement(PowerMode::NormalMode);

        thread::sleep(Duration::from_secs(8));
    }

    Ok(())
}


fn wifi(
    netif_stack: Arc<EspNetifStack>,
    sys_loop_stack: Arc<EspSysLoopStack>,
    default_nvs: Arc<EspDefaultNvs>,
    wifi_ssid : &str,
    wifi_password :&str,
) -> anyhow::Result<Box<EspWifi>> {
    let mut wifi = Box::new(EspWifi::new(netif_stack, sys_loop_stack, default_nvs)?);

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: wifi_ssid.into(),
        password: wifi_password.into(),
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
    } 
    else {
        bail!("Unexpected Wifi status: {:?}", status);
    }

    Ok(wifi)
}