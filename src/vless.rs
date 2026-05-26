use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use anyhow::{anyhow, Result};
use bytes::BytesMut;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub user_id: [u8; 16],
    pub command: u8,
    pub destination: Destination,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    pub address: DestAddr,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestAddr {
    Ip(IpAddr),
    Domain(String),
}

pub fn try_parse(buf: &BytesMut) -> Result<Option<(Header, usize)>> {
    let b = buf.as_ref();
    if b.len() < 18 {
        return Ok(None);
    }

    let version = b[0];
    let user_id: [u8; 16] = b[1..17].try_into().unwrap();

    let addons_len = b[17] as usize;
    let mut i = 18usize;
    if b.len() < i + addons_len + 4 {
        return Ok(None);
    }
    i += addons_len;

    let command = b[i];
    let port = u16::from_be_bytes([b[i + 1], b[i + 2]]);
    let addr_type = b[i + 3];
    i += 4;

    let (address, consumed) = match addr_type {
        0x01 => {
            if b.len() < i + 4 {
                return Ok(None);
            }
            let ip = IpAddr::V4(Ipv4Addr::new(b[i], b[i + 1], b[i + 2], b[i + 3]));
            (DestAddr::Ip(ip), i + 4)
        }
        0x02 => {
            if b.len() < i + 1 {
                return Ok(None);
            }
            let n = b[i] as usize;
            if b.len() < i + 1 + n {
                return Ok(None);
            }
            let name = std::str::from_utf8(&b[i + 1..i + 1 + n])
                .map_err(|_| anyhow!("invalid domain utf8"))?
                .to_string();
            (DestAddr::Domain(name), i + 1 + n)
        }
        0x03 => {
            if b.len() < i + 16 {
                return Ok(None);
            }
            let raw: [u8; 16] = b[i..i + 16].try_into().unwrap();
            let ip = IpAddr::V6(Ipv6Addr::from(raw));
            (DestAddr::Ip(ip), i + 16)
        }
        _ => return Err(anyhow!("unknown addr type: {addr_type:#x}")),
    };

    Ok(Some((
        Header {
            version,
            user_id,
            command,
            destination: Destination { address, port },
        },
        consumed,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    fn base(uuid: [u8; 16]) -> Vec<u8> {
        let mut v = Vec::new();
        v.push(0);
        v.extend_from_slice(&uuid);
        v.push(0);
        v.push(0x01);
        v.extend_from_slice(&80u16.to_be_bytes());
        v
    }

    #[test]
    fn parse_ipv4() {
        let uuid = [7u8; 16];
        let mut v = base(uuid);
        v.push(0x01);
        v.extend_from_slice(&[1, 2, 3, 4]);
        v.extend_from_slice(b"hello");

        let buf = BytesMut::from(&v[..]);
        let (h, n) = try_parse(&buf).unwrap().unwrap();
        assert_eq!(n, 18 + 0 + 4 + 4);
        assert_eq!(h.user_id, uuid);
        assert_eq!(h.command, 0x01);
        assert_eq!(h.destination.port, 80);
        assert_eq!(h.destination.address, DestAddr::Ip(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
    }

    #[test]
    fn parse_domain() {
        let uuid = [9u8; 16];
        let mut v = base(uuid);
        v.push(0x02);
        v.push(3);
        v.extend_from_slice(b"abc");
        let buf = BytesMut::from(&v[..]);
        let (h, n) = try_parse(&buf).unwrap().unwrap();
        assert_eq!(n, 26);
        assert_eq!(h.destination.address, DestAddr::Domain("abc".to_string()));
    }

    #[test]
    fn parse_ipv6() {
        let uuid = [1u8; 16];
        let mut v = base(uuid);
        v.push(0x03);
        v.extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let buf = BytesMut::from(&v[..]);
        let (h, n) = try_parse(&buf).unwrap().unwrap();
        assert_eq!(n, 18 + 0 + 4 + 16);
        match h.destination.address {
            DestAddr::Ip(IpAddr::V6(_)) => {}
            _ => panic!("expected v6"),
        }
    }

    #[test]
    fn partial() {
        let uuid = [7u8; 16];
        let mut v = base(uuid);
        v.push(0x01);
        v.extend_from_slice(&[1, 2, 3, 4]);
        let buf = BytesMut::from(&v[..10]);
        assert!(try_parse(&buf).unwrap().is_none());
    }
}
