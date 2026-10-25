//! STUN (Session Traversal Utilities for NAT) resolver RFC 5389
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_SUCCESS: u16 = 0x0101;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;

fn create_stun_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut message = Vec::with_capacity(20);
    message.extend_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    message.extend_from_slice(&0u16.to_be_bytes()); // Message length must be 0 (no attributes)
    message.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    message.extend_from_slice(transaction_id);
    message
}

fn parse_stun_response(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != STUN_BINDING_SUCCESS {
        return None;
    }
    let magic_cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if magic_cookie != STUN_MAGIC_COOKIE {
        return None;
    }

    let mut offset = 20;
    while offset + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let attr_data_start = offset + 4;

        if attr_data_start + attr_length > data.len() {
            break;
        }

        if attr_type == STUN_ATTR_XOR_MAPPED_ADDRESS || attr_type == STUN_ATTR_MAPPED_ADDRESS {
            if attr_length >= 8 {
                let family = data[attr_data_start + 1]; // family is byte 1 (byte 0 is reserved)
                if family == 0x01 { // IPv4
                    let raw_port = u16::from_be_bytes([data[attr_data_start + 2], data[attr_data_start + 3]]);
                    let raw_ip = [
                        data[attr_data_start + 4],
                        data[attr_data_start + 5],
                        data[attr_data_start + 6],
                        data[attr_data_start + 7],
                    ];

                    let (port, ip) = if attr_type == STUN_ATTR_XOR_MAPPED_ADDRESS {
                        let port = raw_port ^ ((STUN_MAGIC_COOKIE >> 16) as u16);
                        let cookie_bytes = STUN_MAGIC_COOKIE.to_be_bytes();
                        let ip_bytes = [
                            raw_ip[0] ^ cookie_bytes[0],
                            raw_ip[1] ^ cookie_bytes[1],
                            raw_ip[2] ^ cookie_bytes[2],
                            raw_ip[3] ^ cookie_bytes[3],
                        ];
                        (port, std::net::Ipv4Addr::from(ip_bytes))
                    } else {
                        (raw_port, std::net::Ipv4Addr::from(raw_ip))
                    };

                    return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
                }
            }
        }

        // 4-byte padding boundary
        offset += 4 + attr_length;
        if offset % 4 != 0 {
            offset += 4 - (offset % 4);
        }
    }

    None
}

/// Выполняет STUN-запрос через переданный сокет
pub fn get_external_addr(socket: &UdpSocket) -> Option<SocketAddr> {
    let mut transaction_id = [0u8; 12];
    for b in &mut transaction_id {
        *b = rand::random::<u8>();
    }
    let request = create_stun_request(&transaction_id);

    for server in STUN_SERVERS {
        if let Ok(mut addrs) = server.to_socket_addrs() {
            if let Some(target) = addrs.next() {
                let _ = socket.set_read_timeout(Some(Duration::from_millis(1500)));
                if socket.send_to(&request, target).is_ok() {
                    let mut buf = [0u8; 1024];
                    if let Ok((len, _)) = socket.recv_from(&mut buf) {
                        if let Some(ext_addr) = parse_stun_response(&buf[..len]) {
                            return Some(ext_addr);
                        }
                    }
                }
            }
        }
    }
    None
}