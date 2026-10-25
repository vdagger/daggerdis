//! Модуль для автоматического проброса портов через UPnP (IGD).

use std::time::Duration;
use std::net::{Ipv4Addr, SocketAddrV4, SocketAddr, IpAddr, UdpSocket};
use igd_next::{search_gateway, PortMappingProtocol, SearchOptions};

/// Определяет локальный IPv4 адрес сетевого адаптера, через который виден конкретный роутер
fn get_local_ip_for_gateway(gateway_addr: SocketAddr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(gateway_addr).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// Выполняет поиск домашнего роутера и открывает UDP-порт
pub fn forward_port(port: u16) {
    let options = SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    };

    println!("[UPnP] Поиск домашнего роутера в локальной сети...");
    match search_gateway(options) {
        Ok(gateway) => {
            // Определяем локальный IP строго относительно найденного роутера (исключая WSL и виртуальные адаптеры)
            let local_ip = match get_local_ip_for_gateway(gateway.addr) {
                Some(ip) => ip,
                None => {
                    eprintln!("[UPnP] Не удалось определить локальный IP для связи с роутером {}.", gateway.addr);
                    return;
                }
            };

            let local_socket = SocketAddrV4::new(local_ip, port);
            println!("[UPnP] Роутер найден ({}). Локальный адрес ПК: {}", gateway.addr, local_socket);

            // Пробуем аренду 3600 сек (1 час) — стандарт для домашних роутеров
            let lease_time = 3600;
            match gateway.add_port(
                PortMappingProtocol::UDP,
                port,
                SocketAddr::V4(local_socket),
                lease_time,
                "DiscordRF Walkie-Talkie",
            ) {
                Ok(_) => {
                    println!(
                        "[UPnP] УСПЕХ! Роутер автоматически открыл внешний порт {} (UDP) для {}!",
                        port, local_socket
                    );
                }
                Err(e) => {
                    // Если роутер не принял 3600, пробуем 0 (постоянный) как резервный вариант
                    if let Ok(_) = gateway.add_port(
                        PortMappingProtocol::UDP,
                        port,
                        SocketAddr::V4(local_socket),
                        0,
                        "DiscordRF Walkie-Talkie",
                    ) {
                        println!(
                            "[UPnP] УСПЕХ! Роутер открыл порт {} (UDP) с постоянной арендой!",
                            port
                        );
                    } else {
                        eprintln!("[UPnP] Роутер отклонил запрос: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            println!(
                "[UPnP] Роутер не ответил на UPnP-запрос: {}. Если нужно подключение из интернета, используйте VPN (Radmin/Tailscale).",
                e
            );
        }
    }
}