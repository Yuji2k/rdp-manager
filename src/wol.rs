//! Wake-on-LAN: Rechner per „Magic Packet“ aufwecken und optional warten, bis er erreichbar ist.

use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// "aa:bb:cc:dd:ee:ff", "AA-BB-…", "aabb.ccdd.eeff" oder "aabbccddeeff"
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let hex: String = s.chars().filter(|c| !matches!(c, ':' | '-' | '.' | ' ')).collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, b) in mac.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

pub fn format_mac(mac: [u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// 6 × 0xFF, danach 16 × die MAC-Adresse
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut p = [0xFFu8; 102];
    for i in 0..16 {
        p[6 + i * 6..12 + i * 6].copy_from_slice(&mac);
    }
    p
}

fn ipv4_of(host: &str) -> Option<Ipv4Addr> {
    let host = host.trim();
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    (host, 0).to_socket_addrs().ok()?.find_map(|a| match a {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        _ => None,
    })
}

/// Magic Packet senden: an alle (255.255.255.255) und – falls der Host auflösbar ist – an die
/// Broadcast-Adresse seines /24-Netzes (für Router, die gerichtete Broadcasts weiterleiten).
pub fn send(mac: [u8; 6], host: &str) -> Result<(), String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("Wake-on-LAN: {e}"))?;
    sock.set_broadcast(true).map_err(|e| format!("Wake-on-LAN: {e}"))?;
    let packet = magic_packet(mac);
    let mut targets = vec![Ipv4Addr::BROADCAST];
    if let Some(ip) = ipv4_of(host) {
        let o = ip.octets();
        targets.push(Ipv4Addr::new(o[0], o[1], o[2], 255));
    }
    let mut sent = false;
    for t in targets {
        for port in [9, 7] {
            sent |= sock.send_to(&packet, (t, port)).is_ok();
        }
    }
    if sent { Ok(()) } else { Err("Wake-on-LAN-Paket konnte nicht gesendet werden.".into()) }
}

/// MAC-Adresse eines Rechners im selben Netz per ARP ermitteln
pub fn lookup_mac(host: &str) -> Result<String, String> {
    use windows::Win32::NetworkManagement::IpHelper::SendARP;
    let ip = ipv4_of(host).ok_or_else(|| format!("„{host}“ lässt sich nicht in eine IPv4-Adresse auflösen."))?;
    let mut mac = [0u8; 8];
    let mut len = mac.len() as u32;
    let r = unsafe { SendARP(u32::from_ne_bytes(ip.octets()), 0, mac.as_mut_ptr() as *mut std::ffi::c_void, &mut len) };
    if r != 0 || len < 6 {
        return Err(format!(
            "Keine Antwort von {ip} – die MAC-Adresse lässt sich nur ermitteln, wenn der Rechner eingeschaltet ist und im selben Netz liegt."
        ));
    }
    Ok(format_mac([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]]))
}

/// Wecken und warten, bis `host:port` antwortet (Paket alle 30 s erneut). Ergebnis: erreichbar?
pub fn wake_and_wait(mac: [u8; 6], host: &str, port: u16, timeout: Duration, ctx: &eframe::egui::Context) -> mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    let (host, ctx) = (host.trim().to_string(), ctx.clone());
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut last_send: Option<Instant> = None;
        let result = loop {
            if last_send.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                if let Err(e) = send(mac, &host) {
                    break Err(e);
                }
                last_send = Some(Instant::now());
            }
            let up = (host.as_str(), port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(2)).is_ok());
            if up {
                break Ok(());
            }
            if start.elapsed() > timeout {
                break Err(format!("{host} ist nach {} s noch nicht erreichbar.", timeout.as_secs()));
            }
            std::thread::sleep(Duration::from_secs(3));
        };
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_and_packet() {
        let m = [0x00, 0x1A, 0x2B, 0x3C, 0x4D, 0x5E];
        for s in ["00:1a:2b:3c:4d:5e", "00-1A-2B-3C-4D-5E", "001a.2b3c.4d5e", "001A2B3C4D5E"] {
            assert_eq!(parse_mac(s), Some(m), "{s}");
        }
        assert_eq!(parse_mac("00:1a:2b"), None);
        assert_eq!(parse_mac("zz:1a:2b:3c:4d:5e"), None);
        assert_eq!(format_mac(m), "00:1A:2B:3C:4D:5E");
        let p = magic_packet(m);
        assert_eq!(&p[..6], &[0xFF; 6]);
        assert_eq!(&p[6..12], &m);
        assert_eq!(&p[96..102], &m);
    }
}
