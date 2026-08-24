use maxminddb::{Reader, geoip2};
use std::net::IpAddr;

pub fn test(reader: &Reader<Vec<u8>>, ip: IpAddr) {
    let _rec: geoip2::Country = reader.lookup(ip).unwrap();
}
