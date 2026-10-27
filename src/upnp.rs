//! Модуль автоматического проброса портов через UPnP (IGD)

use std::net::{SocketAddrV4, Ipv4Addr};

/// Пытается автоматически открыть UDP-порт на домашнем роутере
pub fn auto_forward_port(local_port: u16) {
    std::thread::spawn(move || {
        println!("[UPnP] Поиск домашнего роутера в локальной сети...");
        match igd::search_gateway(Default::default()) {
            Ok(gateway) => {
                println!("[UPnP] Найден роутер: {}", gateway);

                // Определяем локальный IP-адрес интерфейса, смотрящего на роутер
                let local_ip = match std::net::UdpSocket::bind("0.0.0.0:0") {
                    Ok(s) => {
                        let target = std::net::SocketAddr::V4(gateway.addr);
                        if s.connect(target).is_ok() {
                            match s.local_addr() {
                                Ok(std::net::SocketAddr::V4(addr)) => Some(*addr.ip()),
                                _ => None,
                            }
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };

                if let Some(local_ipv4) = local_ip {
                    let local_dest = SocketAddrV4::new(local_ipv4, local_port);
                    println!("[UPnP] Запрос на проброс порта {} -> {}...", local_port, local_dest);

                    match gateway.add_port(
                        igd::PortMappingProtocol::UDP,
                        local_port,
                        local_dest,
                        7200, // Аренда на 2 часа
                        "DiscordRF",
                    ) {
                        Ok(()) => {
                            println!("[UPnP] 🚀 Порт {} успешно открыт на роутере!", local_port);
                        }
                        Err(e) => {
                            println!("[UPnP] Роутер отклонил открытие порта (возможно, ограничение прошивки): {}", e);
                        }
                    }
                } else {
                    println!("[UPnP] Не удалось определить локальный IP для роутера.");
                }
            }
            Err(e) => {
                println!("[UPnP] Роутер не ответил на UPnP-запрос (будет использоваться STUN-пробив): {}", e);
            }
        }
    });
}