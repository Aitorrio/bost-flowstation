// IPv4 / UDP / TCP build + parse for the SNDCP WAP data path.
//
// Pure byte-vector helpers (not BitBuffer-native): an SN-UNITDATA / SN-DATA SDU carries a raw IP
// datagram as its N-PDU, octet-aligned, so once the N-PDU bytes are in hand everything here is plain
// `&[u8]` / `Vec<u8>`. Build constants and layouts follow the WAP port spec (Docs/wap-port-spec.md
// §2.4-2.6, §3). RFC 1071 Internet checksum; UDP checksum is always written as 0x0000; IPv4 and TCP
// checksums are computed. Parsers honour IHL / header length and the IP Total Length field, and do
// NOT verify checksums (a receiver trusts the SwMI link).

/// IP protocol numbers (IPv4 header byte 9).
pub const IPV4_PROTOCOL_TCP: u8 = 6;
pub const IPV4_PROTOCOL_UDP: u8 = 17;

/// Minimum header lengths (octets).
pub const IPV4_MIN_HEADER_LEN: usize = 20;
pub const TCP_MIN_HEADER_LEN: usize = 20;
pub const UDP_HEADER_LEN: usize = 8;

/// IP version nibbles (first nibble of the N-PDU).
pub const IPV4_VERSION: u8 = 4;
pub const IPV6_VERSION: u8 = 6;

/// IPv4 build constants (spec §3).
const IPV4_BYTE0_VERSION_IHL: u8 = 0x45; // version 4, IHL 5 (no options)
const IPV4_BYTE1_DSCP_ECN: u8 = 0x00;
/// TCP DataOffset=5 occupies the high nibble of byte 12; `0x50` base before the NS bit.
const TCP_BYTE12_DATA_OFFSET_5: u8 = 0x50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpError {
    Ipv4TooShort,
    NotIpv4,
    UdpTooShort,
    TcpHeaderTooShort,
}

/// RFC 1071 one's-complement Internet checksum over `data`, treated as big-endian 16-bit words.
/// A trailing odd byte is padded with a zero low byte.
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | (data[i + 1] as u32);
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Checksum of an IPv4 header (caller supplies the full 20-byte header; the checksum field must be
/// zeroed first by the builder). Re-running this over a header that already carries its checksum
/// yields 0.
pub fn ipv4_header_checksum(header: &[u8]) -> u16 {
    internet_checksum(header)
}

/// TCP checksum over the pseudo-header (`src ++ dst ++ {0x00, 0x06} ++ be16(seg_len)`) + segment.
/// Re-running over a segment that already carries its checksum yields 0.
pub fn tcp_checksum(src: [u8; 4], dst: [u8; 4], segment: &[u8]) -> u16 {
    let mut buf = Vec::with_capacity(12 + segment.len());
    buf.extend_from_slice(&src);
    buf.extend_from_slice(&dst);
    buf.push(0x00);
    buf.push(IPV4_PROTOCOL_TCP);
    buf.extend_from_slice(&(segment.len() as u16).to_be_bytes());
    buf.extend_from_slice(segment);
    internet_checksum(&buf)
}

/// Build a 20-byte IPv4 header (no options) + payload. `id` is the Identification field; for WAP
/// responses the caller passes `request_id + 1` (wrapping). Header checksum computed, payload
/// appended verbatim.
pub fn build_ipv4(src: [u8; 4], dst: [u8; 4], protocol: u8, id: u16, ttl: u8, payload: &[u8]) -> Vec<u8> {
    let total = (IPV4_MIN_HEADER_LEN + payload.len()) as u16;
    let mut pkt = vec![0u8; IPV4_MIN_HEADER_LEN];
    pkt[0] = IPV4_BYTE0_VERSION_IHL;
    pkt[1] = IPV4_BYTE1_DSCP_ECN;
    pkt[2..4].copy_from_slice(&total.to_be_bytes());
    pkt[4..6].copy_from_slice(&id.to_be_bytes());
    // bytes 6..8 flags/fragment-offset = 0x0000 (already zero)
    pkt[8] = ttl;
    pkt[9] = protocol;
    // bytes 10..12 checksum left zero for the computation
    pkt[12..16].copy_from_slice(&src);
    pkt[16..20].copy_from_slice(&dst);
    let ck = ipv4_header_checksum(&pkt);
    pkt[10..12].copy_from_slice(&ck.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

/// Build a UDP header + payload. Checksum field is always 0x0000 (never computed) per spec §2.5.
pub fn build_udp(src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = (UDP_HEADER_LEN + payload.len()) as u16;
    let mut seg = Vec::with_capacity(UDP_HEADER_LEN + payload.len());
    seg.extend_from_slice(&src_port.to_be_bytes());
    seg.extend_from_slice(&dst_port.to_be_bytes());
    seg.extend_from_slice(&len.to_be_bytes());
    seg.extend_from_slice(&0u16.to_be_bytes()); // checksum 0x0000, always
    seg.extend_from_slice(payload);
    seg
}

/// Build a complete IPv4/UDP datagram.
pub fn build_ipv4_udp(
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    id: u16,
    ttl: u8,
    payload: &[u8],
) -> Vec<u8> {
    let udp = build_udp(src_port, dst_port, payload);
    build_ipv4(src, dst, IPV4_PROTOCOL_UDP, id, ttl, &udp)
}

/// Build a TCP segment (20-byte header, no options) with computed checksum. `flags` is the 9-bit
/// flag field (NS is bit 8); byte 12 = DataOffset 5 | NS, byte 13 = low 8 flag bits (spec §2.6).
pub fn build_tcp_segment(
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u16,
    window: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut seg = vec![0u8; TCP_MIN_HEADER_LEN];
    seg[0..2].copy_from_slice(&src_port.to_be_bytes());
    seg[2..4].copy_from_slice(&dst_port.to_be_bytes());
    seg[4..8].copy_from_slice(&seq.to_be_bytes());
    seg[8..12].copy_from_slice(&ack.to_be_bytes());
    seg[12] = TCP_BYTE12_DATA_OFFSET_5 | (((flags >> 8) & 1) as u8);
    seg[13] = (flags & 0xff) as u8;
    seg[14..16].copy_from_slice(&window.to_be_bytes());
    // bytes 16..18 checksum zero for the computation; bytes 18..20 urgent pointer 0x0000
    seg.extend_from_slice(payload);
    let ck = tcp_checksum(src, dst, &seg);
    seg[16..18].copy_from_slice(&ck.to_be_bytes());
    seg
}

/// Build a complete IPv4/TCP datagram.
#[allow(clippy::too_many_arguments)]
pub fn build_ipv4_tcp(
    src: [u8; 4],
    dst: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u16,
    window: u16,
    id: u16,
    ttl: u8,
    payload: &[u8],
) -> Vec<u8> {
    let seg = build_tcp_segment(src, dst, src_port, dst_port, seq, ack, flags, window, payload);
    build_ipv4(src, dst, IPV4_PROTOCOL_TCP, id, ttl, &seg)
}

/// A parsed IPv4 datagram. `payload` is the transport segment, trimmed to the Total Length field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4<'a> {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub protocol: u8,
    pub id: u16,
    pub ttl: u8,
    pub payload: &'a [u8],
}

/// Parse an IPv4 datagram. Honours IHL and trims to Total Length; does NOT verify the checksum.
pub fn parse_ipv4(packet: &[u8]) -> Result<Ipv4<'_>, IpError> {
    if packet.len() < IPV4_MIN_HEADER_LEN {
        return Err(IpError::Ipv4TooShort);
    }
    if packet[0] >> 4 != IPV4_VERSION {
        return Err(IpError::NotIpv4);
    }
    let ihl = ((packet[0] & 0x0f) as usize) * 4;
    if ihl < IPV4_MIN_HEADER_LEN || packet.len() < ihl {
        return Err(IpError::Ipv4TooShort);
    }
    let total = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    let end = total.clamp(ihl, packet.len());
    let mut src = [0u8; 4];
    let mut dst = [0u8; 4];
    src.copy_from_slice(&packet[12..16]);
    dst.copy_from_slice(&packet[16..20]);
    Ok(Ipv4 {
        src,
        dst,
        protocol: packet[9],
        id: u16::from_be_bytes([packet[4], packet[5]]),
        ttl: packet[8],
        payload: &packet[ihl..end],
    })
}

/// A parsed UDP segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

/// Parse a UDP segment. Trims the payload to the UDP Length field when it is within bounds.
pub fn parse_udp(segment: &[u8]) -> Result<Udp<'_>, IpError> {
    if segment.len() < UDP_HEADER_LEN {
        return Err(IpError::UdpTooShort);
    }
    let len = u16::from_be_bytes([segment[4], segment[5]]) as usize;
    let end = len.clamp(UDP_HEADER_LEN, segment.len());
    Ok(Udp {
        src_port: u16::from_be_bytes([segment[0], segment[1]]),
        dst_port: u16::from_be_bytes([segment[2], segment[3]]),
        payload: &segment[UDP_HEADER_LEN..end],
    })
}

/// A parsed TCP segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tcp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u16,
    pub window: u16,
    pub payload: &'a [u8],
}

/// Parse a TCP segment. Honours the Data Offset field for the header length.
pub fn parse_tcp(segment: &[u8]) -> Result<Tcp<'_>, IpError> {
    if segment.len() < TCP_MIN_HEADER_LEN {
        return Err(IpError::TcpHeaderTooShort);
    }
    let data_offset = ((segment[12] >> 4) as usize) * 4;
    if data_offset < TCP_MIN_HEADER_LEN || segment.len() < data_offset {
        return Err(IpError::TcpHeaderTooShort);
    }
    let flags = (((segment[12] & 0x01) as u16) << 8) | segment[13] as u16;
    Ok(Tcp {
        src_port: u16::from_be_bytes([segment[0], segment[1]]),
        dst_port: u16::from_be_bytes([segment[2], segment[3]]),
        seq: u32::from_be_bytes([segment[4], segment[5], segment[6], segment[7]]),
        ack: u32::from_be_bytes([segment[8], segment[9], segment[10], segment[11]]),
        flags,
        window: u16::from_be_bytes([segment[14], segment[15]]),
        payload: &segment[data_offset..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // §7.3 Vector A: IPv4/UDP, 78-byte payload. Header (20) + UDP (8) bytes are independent of the
    // payload content, so a dummy 78-byte payload reproduces the ground-truth 28-byte prefix.
    #[test]
    fn vector_a_ipv4_udp_header_prefix() {
        let pkt = build_ipv4_udp([10, 0, 0, 1], [10, 0, 0, 226], 9200, 9200, 0x1234, 64, &[0u8; 78]);
        let want_ip = hex("45 00 00 6a 12 34 00 00 40 11 53 6d 0a 00 00 01 0a 00 00 e2");
        let want_udp = hex("23 f0 23 f0 00 56 00 00");
        assert_eq!(&pkt[..20], &want_ip[..], "IPv4 header");
        assert_eq!(&pkt[20..28], &want_udp[..], "UDP header");
    }

    // §7.3 Vector B: full 31-byte IPv4/UDP packet carrying "wap". Exact match incl. checksum 0x16c4.
    #[test]
    fn vector_b_ipv4_udp_full_packet() {
        let pkt = build_ipv4_udp([192, 0, 2, 1], [192, 0, 2, 2], 49152, 9200, 7, 32, b"wap");
        let want = hex("4500001f 00070000 201116c4 c0000201 c0000202 c00023f0 000b0000 776170");
        assert_eq!(pkt, want);
    }

    // §7.3 Vector C: full IPv4/TCP packet. Exact match incl. IP cksum 0x53a7, TCP cksum 0xcbbe.
    #[test]
    fn vector_c_ipv4_tcp_full_packet() {
        let pkt = build_ipv4_tcp(
            [10, 0, 0, 1],
            [10, 0, 0, 226],
            9200,
            49152,
            0x1122_3344,
            0x5566_7788,
            0x019, // FIN + PSH + ACK
            0x1000,
            0x1234,
            64,
            b"HTTP/1.0 200 OK\r\n\r\n",
        );
        let want = hex(
            "4500003b12340000400653a70a0000010a0000e2\
             23f0c000112233445566778850191000cbbe0000\
             485454502f312e3020323030204f4b0d0a0d0a",
        );
        assert_eq!(pkt, want);
    }

    // Re-running the checksum over a built header/segment yields 0 (spec §7.3).
    #[test]
    fn checksums_verify_to_zero() {
        let pkt = build_ipv4_udp([192, 0, 2, 1], [192, 0, 2, 2], 49152, 9200, 7, 32, b"wap");
        assert_eq!(ipv4_header_checksum(&pkt[..20]), 0);
        let seg = build_tcp_segment(
            [10, 0, 0, 1], [10, 0, 0, 226], 9200, 49152, 0x1122_3344, 0x5566_7788, 0x019, 0x1000,
            b"HTTP/1.0 200 OK\r\n\r\n",
        );
        assert_eq!(tcp_checksum([10, 0, 0, 1], [10, 0, 0, 226], &seg), 0);
    }

    #[test]
    fn parse_round_trips_udp() {
        let pkt = build_ipv4_udp([192, 0, 2, 1], [192, 0, 2, 2], 49152, 9200, 7, 32, b"wap");
        let ip = parse_ipv4(&pkt).unwrap();
        assert_eq!(ip.src, [192, 0, 2, 1]);
        assert_eq!(ip.dst, [192, 0, 2, 2]);
        assert_eq!(ip.protocol, IPV4_PROTOCOL_UDP);
        assert_eq!(ip.id, 7);
        let udp = parse_udp(ip.payload).unwrap();
        assert_eq!(udp.src_port, 49152);
        assert_eq!(udp.dst_port, 9200);
        assert_eq!(udp.payload, b"wap");
    }

    #[test]
    fn parse_round_trips_tcp() {
        let pkt = build_ipv4_tcp(
            [10, 0, 0, 1], [10, 0, 0, 226], 9200, 49152, 0x1122_3344, 0x5566_7788, 0x019, 0x1000,
            0x1234, 64, b"HTTP/1.0 200 OK\r\n\r\n",
        );
        let ip = parse_ipv4(&pkt).unwrap();
        assert_eq!(ip.protocol, IPV4_PROTOCOL_TCP);
        let tcp = parse_tcp(ip.payload).unwrap();
        assert_eq!(tcp.seq, 0x1122_3344);
        assert_eq!(tcp.ack, 0x5566_7788);
        assert_eq!(tcp.flags, 0x019);
        assert_eq!(tcp.window, 0x1000);
        assert_eq!(tcp.payload, b"HTTP/1.0 200 OK\r\n\r\n");
    }

    #[test]
    fn parse_errors() {
        assert_eq!(parse_ipv4(&[0u8; 19]), Err(IpError::Ipv4TooShort));
        assert_eq!(parse_udp(&[0u8; 7]), Err(IpError::UdpTooShort));
        let mut tcp = vec![0u8; 20];
        tcp[12] = 4 << 4; // data offset 4 (<5) -> too short
        assert_eq!(parse_tcp(&tcp), Err(IpError::TcpHeaderTooShort));
    }
}
