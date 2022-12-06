use std::time::Duration;
use anyhow::*;
use log::*;
use std::result::Result::Ok;

// Common IDF stuff 
use esp_idf_hal::prelude::*;
use esp_idf_hal::*;
use esp_idf_sys::*;

// Peripheral stuff
use esp_idf_hal::{prelude::Peripherals};

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
use esp_idf_svc::netif::*;
use esp_idf_svc::eventloop::*;
use esp_idf_svc::wifi::*;
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

    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;

    let i2c = peripherals.i2c0;
    let sda = peripherals.pins.gpio10;
    let scl = peripherals.pins.gpio8;

    let config = <i2c::config::Config>::default().baudrate(100.kHz().into());
    let i2c = i2c::I2cDriver::new(i2c, sda, scl, &config)?;


    let bus = BusManagerSimple::new(i2c);
    //let mut icm = Icm42670::new(bus.acquire_i2c(), Address::Primary).unwrap(); // in case you want to collect gyroscope and accelerometer data
    let mut sht = shtc3(bus.acquire_i2c());
    
    /* If your code is panicking here, consider using LowPower mode since NormalMode may cause 
        code panicking if you're trying to take measurements "too early" */
    sht.start_measurement(PowerMode::NormalMode);


    info!("About to initialize WiFi (SSID: {}, PASS: {})", app_config.wifi_ssid, app_config.wifi_pass);    
    let mut wifi = wifi(peripherals.modem, sysloop.clone(), app_config.wifi_ssid, app_config.wifi_pass);

    
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

    client.subscribe(app_config.topic_name, QoS::AtLeastOnce);

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
    modem: impl peripheral::Peripheral<P = esp_idf_hal::modem::Modem> + 'static,
    sysloop: EspSystemEventLoop,
    wifi_ssid : &str, 
    wifi_password : &str, 
) -> Result<Box<EspWifi<'static>>> {
    use std::net::Ipv4Addr;

    let mut wifi = Box::new(EspWifi::new(modem, sysloop.clone(), None)?);

    info!("Wifi created, about to scan");

    let ap_infos = wifi.scan()?;

    let ours = ap_infos.into_iter().find(|a| a.ssid == wifi_ssid);

    let channel = if let Some(ours) = ours {
        info!(
            "Found configured access point {} on channel {}",
            wifi_ssid, ours.channel
        );
        Some(ours.channel)
    } else {
        info!(
            "Configured access point {} not found during scanning, will go with unknown channel",
            wifi_ssid
        );
        None
    };

    wifi.set_configuration(&Configuration::Mixed(
        ClientConfiguration {
            ssid: wifi_ssid.into(),
            password: wifi_password.into(),
            channel,
            ..Default::default()
        },
        AccessPointConfiguration {
            ssid: "aptest".into(),
            channel: channel.unwrap_or(1),
            ..Default::default()
        },
    ))?;

    wifi.start()?;

    info!("Starting wifi...");

    if !WifiWait::new(&sysloop)?
        .wait_with_timeout(Duration::from_secs(20), || wifi.is_started().unwrap())
    {
        bail!("Wifi did not start");
    }

    info!("Connecting wifi...");

    wifi.connect()?;

    if !EspNetifWait::new::<EspNetif>(wifi.sta_netif(), &sysloop)?.wait_with_timeout(
        Duration::from_secs(20),
        || {
            wifi.is_connected().unwrap()
                && wifi.sta_netif().get_ip_info().unwrap().ip != Ipv4Addr::new(0, 0, 0, 0)
        },
    ) {
        bail!("Wifi did not connect or did not receive a DHCP lease");
    }

    let ip_info = wifi.sta_netif().get_ip_info()?;

    info!("Wifi DHCP info: {:?}", ip_info);

    // ping(ip_info.subnet.gateway)?;

    Ok(wifi)
}