// WTP (class 2) + WSP transaction layer for the SNDCP WAP data path.
//
// Sits on top of `ip` (the IPv4/UDP transport). A terminal's WAP browser (Openwave UP.Browser on
// Motorola/Dimetra, etc.) sends a UDP datagram to the BS WAP port carrying either a WTP Invoke
// wrapping a WSP PDU (Connect / GET / Resume), a WTP control PDU (ACK / ABORT — no response), or a
// plain-text "GET /…". The BS classifies the request, renders a reply, and returns a WTP Result
// wrapping the WSP reply in a UDP datagram with source/destination swapped and IP id = req_id + 1.
//
// All layouts and constants follow the WAP port spec (Docs/wap-port-spec.md §2.7-2.11, §3); the
// §7.4 capture vectors are frozen as tests. This is a clean-room reimplementation from that spec.

use super::ip;

// ---- WTP (spec §2.7-2.8, §3) ------------------------------------------------------------------

const WTP_CON_FLAG: u8 = 0x80;
const WTP_RID_FLAG: u8 = 0x01;
const WTP_PDU_INVOKE: u8 = 1;
const WTP_PDU_RESULT: u8 = 2;
const WTP_PDU_ACK: u8 = 3;
const WTP_PDU_ABORT: u8 = 4;
/// Result PDU, last-packet trailer, RID clear: `(2 << 3) | 0x02`.
const WTP_RESULT_LAST_PACKET: u8 = 0x12;
const WTP_TID_RESPONSE_FLAG: u16 = 0x8000;
const WTP_TID_VALUE_MASK: u16 = 0x7fff;

// ---- WSP (spec §2.9-2.11, §3) -----------------------------------------------------------------

const WSP_PDU_CONNECT: u8 = 0x01;
const WSP_PDU_CONNECT_REPLY: u8 = 0x02;
const WSP_PDU_REPLY: u8 = 0x04;
const WSP_PDU_RESUME: u8 = 0x09;
const WSP_PDU_GET: u8 = 0x40; // Get method family 0x40..=0x5f
const WSP_STATUS_OK: u8 = 0x20;
/// Content-type short-integers (never a textual MIME string).
pub const WSP_CT_WML: u8 = 0x88; // 0x80 | 0x08
pub const WSP_CT_XHTML: u8 = 0xC5; // 0x80 | 0x45
const WSP_CAP_CLIENT_SDU: u8 = 0x80;
const WSP_CAP_SERVER_SDU: u8 = 0x81;
/// Hardcoded server session id in a Connect-Reply.
const WSP_SERVER_SESSION_ID: u8 = 0x01;
/// The "545 cap": Client-/Server-SDU are clamped to this in the Connect-Reply.
const WSP_CONNECT_REPLY_SDU_MAX: u32 = 545;

// ---- uintvar codec (spec §2.11) ---------------------------------------------------------------

/// Encode a big-endian base-128 uintvar: non-final octets have bit 7 set; minimal length, except
/// 0 encodes as a single `0x00`.
pub fn encode_uintvar(value: u32) -> Vec<u8> {
    if value == 0 {
        return vec![0x00];
    }
    let mut groups = Vec::new();
    let mut v = value;
    while v > 0 {
        groups.push((v & 0x7f) as u8);
        v >>= 7;
    }
    groups.reverse();
    let n = groups.len();
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| if i + 1 < n { g | 0x80 } else { *g })
        .collect()
}

/// Decode a uintvar; returns `(value, octets_consumed)`. At most 5 octets. `None` if it runs off the
/// end or exceeds 5 octets without a terminator.
pub fn decode_uintvar(buf: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        if i >= 5 {
            return None;
        }
        value = (value << 7) | (b & 0x7f) as u32;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

// ---- Request classification -------------------------------------------------------------------

/// A classified WAP request (output of [`classify`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapRequest {
    /// WTP Invoke wrapping WSP Connect. `retransmission` = WTP RID bit.
    WtpWspConnect {
        tid: u16,
        retransmission: bool,
        client_sdu: u32,
        server_sdu: u32,
    },
    /// WTP Invoke wrapping WSP GET of `path`.
    WtpWspGet {
        tid: u16,
        retransmission: bool,
        path: String,
    },
    /// WTP Invoke wrapping WSP Resume.
    WtpWspResume { tid: u16, retransmission: bool },
    /// WTP control PDU (ACK / ABORT): no datagram is returned.
    WtpControlNoResponse { pdu: u8 },
    /// Plain-text `GET /path` carried directly in UDP (raw-UDP path, no WTP/WSP).
    TextGet { path: String },
    /// Unrecognised / unsupported payload of the given length.
    Unsupported { len: usize },
}

/// Advance past any WTP TPIs that follow the Invoke header when the CON flag is set (spec §2.7).
/// Returns the offset at which the WSP PDU begins.
fn skip_tpis(buf: &[u8], mut off: usize, con: bool) -> usize {
    if !con {
        return off;
    }
    while off < buf.len() {
        let h = buf[off];
        let long = h & 0x04 != 0;
        let next = if long {
            off + 2 + buf.get(off + 1).copied().unwrap_or(0) as usize
        } else {
            off + 1 + (h & 0x03) as usize
        };
        off = next;
        if h & 0x80 == 0 {
            break; // continuation clear: last TPI
        }
    }
    off
}

/// Extract Client-SDU (0x80) and Server-SDU (0x81) from a WSP Connect capabilities blob.
fn parse_connect_sdus(caps: &[u8]) -> (u32, u32) {
    let (mut client, mut server) = (0u32, 0u32);
    let mut off = 0;
    while off < caps.len() {
        let Some((len, used)) = decode_uintvar(&caps[off..]) else {
            break;
        };
        let len = len as usize;
        let field = off + used;
        if len == 0 || field >= caps.len() || field + len > caps.len() {
            break;
        }
        let id = caps[field];
        let param = &caps[field + 1..field + len];
        if id == WSP_CAP_CLIENT_SDU {
            if let Some((v, _)) = decode_uintvar(param) {
                client = v;
            }
        } else if id == WSP_CAP_SERVER_SDU {
            if let Some((v, _)) = decode_uintvar(param) {
                server = v;
            }
        }
        off = field + len;
    }
    (client, server)
}

/// Classify the WAP request carried in a UDP payload. `<3` byte non-WTP payloads and anything that
/// is neither a recognised WTP PDU nor a text GET become [`WapRequest::Unsupported`].
pub fn classify(payload: &[u8]) -> WapRequest {
    if payload.is_empty() {
        return WapRequest::Unsupported { len: 0 };
    }
    let pdu_type = (payload[0] >> 3) & 0x0f;

    // WTP control PDUs first: ACK / ABORT get no response.
    if pdu_type == WTP_PDU_ACK || pdu_type == WTP_PDU_ABORT {
        return WapRequest::WtpControlNoResponse { pdu: pdu_type };
    }

    if pdu_type == WTP_PDU_INVOKE && payload.len() >= 4 {
        let con = payload[0] & WTP_CON_FLAG != 0;
        let retransmission = payload[0] & WTP_RID_FLAG != 0;
        let tid = u16::from_be_bytes([payload[1], payload[2]]) & WTP_TID_VALUE_MASK;
        // payload[3] = Invoke header (TCL=2, canonical 0x12); WSP follows (after any TPIs).
        let wsp_off = skip_tpis(payload, 4, con);
        if wsp_off < payload.len() {
            let wsp = &payload[wsp_off..];
            match wsp[0] {
                WSP_PDU_CONNECT if wsp.len() >= 2 => {
                    // [0x01][version][CapLen uintvar][HeadersLen uintvar][Caps][Headers]
                    let mut o = 2;
                    let (cap_len, u1) = decode_uintvar(&wsp[o..]).unwrap_or((0, 0));
                    o += u1;
                    let (_hdr_len, u2) = decode_uintvar(&wsp[o..]).unwrap_or((0, 0));
                    o += u2;
                    let cap_end = (o + cap_len as usize).min(wsp.len());
                    let (client_sdu, server_sdu) = parse_connect_sdus(&wsp[o..cap_end]);
                    return WapRequest::WtpWspConnect {
                        tid,
                        retransmission,
                        client_sdu,
                        server_sdu,
                    };
                }
                WSP_PDU_RESUME => return WapRequest::WtpWspResume { tid, retransmission },
                m if (WSP_PDU_GET..=0x5f).contains(&m) => {
                    // [method][uintvar URILen][URI][headers]
                    let path = decode_uintvar(&wsp[1..])
                        .and_then(|(len, used)| {
                            let start = 1 + used;
                            let end = start + len as usize;
                            wsp.get(start..end.min(wsp.len()))
                        })
                        .map(|u| String::from_utf8_lossy(u).into_owned())
                        .unwrap_or_default();
                    return WapRequest::WtpWspGet {
                        tid,
                        retransmission,
                        path,
                    };
                }
                _ => return WapRequest::Unsupported { len: payload.len() },
            }
        }
        return WapRequest::Unsupported { len: payload.len() };
    }

    // Not WTP: a plain-text "GET /path …" (raw-UDP path), else unsupported.
    if payload.len() >= 4 && &payload[..4] == b"GET " {
        let rest = &payload[4..];
        let end = rest
            .iter()
            .position(|&b| b == b' ' || b == b'\r' || b == b'\n')
            .unwrap_or(rest.len());
        return WapRequest::TextGet {
            path: String::from_utf8_lossy(&rest[..end]).into_owned(),
        };
    }

    WapRequest::Unsupported { len: payload.len() }
}

// ---- Response builders ------------------------------------------------------------------------

/// Wrap a WSP PDU in a WTP Result: `[0x12, respTID_be16, wsp…]` where respTID = (reqTID & 0x7fff) |
/// 0x8000 and the RID bit is always clear.
pub fn wtp_result(req_tid: u16, wsp: &[u8]) -> Vec<u8> {
    let resp_tid = (req_tid & WTP_TID_VALUE_MASK) | WTP_TID_RESPONSE_FLAG;
    let mut out = Vec::with_capacity(3 + wsp.len());
    out.push(WTP_RESULT_LAST_PACKET);
    out.extend_from_slice(&resp_tid.to_be_bytes());
    out.extend_from_slice(wsp);
    out
}

/// Encode one Connect-Reply capability: `[uintvar(1+len(val)), id, uintvar(val)]`.
fn connect_reply_cap(id: u8, value: u32) -> Vec<u8> {
    let val = encode_uintvar(value);
    let mut cap = encode_uintvar((1 + val.len()) as u32);
    cap.push(id);
    cap.extend_from_slice(&val);
    cap
}

/// Build the WSP Connect-Reply PDU: `[0x02, 0x01, uintvar(caps_len), 0x00, <caps>]`. Server session
/// id hardcoded 0x01, HeadersLength 0x00; only Client-/Server-SDU echoed, each clamped to 545.
pub fn wsp_connect_reply(client_sdu: u32, server_sdu: u32) -> Vec<u8> {
    let mut caps = connect_reply_cap(WSP_CAP_CLIENT_SDU, client_sdu.min(WSP_CONNECT_REPLY_SDU_MAX));
    caps.extend_from_slice(&connect_reply_cap(
        WSP_CAP_SERVER_SDU,
        server_sdu.min(WSP_CONNECT_REPLY_SDU_MAX),
    ));
    let mut out = vec![WSP_PDU_CONNECT_REPLY, WSP_SERVER_SESSION_ID];
    out.extend_from_slice(&encode_uintvar(caps.len() as u32));
    out.push(0x00); // HeadersLength
    out.extend_from_slice(&caps);
    out
}

/// Build a WSP Reply (GET success): `[0x04, 0x20, 0x01, content_type, body…]`.
pub fn wsp_reply(content_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&[WSP_PDU_REPLY, WSP_STATUS_OK, 0x01, content_type]);
    out.extend_from_slice(body);
    out
}

/// Build a WSP Resume-OK reply: exactly `[0x04, 0x20, 0x00]`.
pub fn wsp_resume_ok() -> Vec<u8> {
    vec![WSP_PDU_REPLY, WSP_STATUS_OK, 0x00]
}

/// The BS WAP endpoint (address/port/ttl the responses originate from).
#[derive(Debug, Clone, Copy)]
pub struct WapEndpoint {
    pub address: [u8; 4],
    pub port: u16,
    pub response_ttl: u8,
}

/// Build the downlink IPv4/UDP datagram for a WAP response: source/destination and ports swapped
/// relative to the request, IP id = request_id + 1 (wrapping), ttl from the endpoint, UDP checksum
/// 0 (spec §5.1).
pub fn build_response_datagram(
    endpoint: &WapEndpoint,
    request: &ip::Ipv4<'_>,
    request_src_port: u16,
    request_ip_id: u16,
    udp_payload: &[u8],
) -> Vec<u8> {
    ip::build_ipv4_udp(
        endpoint.address,
        request.src,
        endpoint.port,
        request_src_port,
        request_ip_id.wrapping_add(1),
        endpoint.response_ttl,
        udp_payload,
    )
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

    // §2.11 / §7.4: 2-octet uintvar 0x82 0x64 = 356; 545 encodes to [0x84, 0x21].
    #[test]
    fn uintvar_codec() {
        assert_eq!(decode_uintvar(&[0x82, 0x64]), Some((356, 2)));
        assert_eq!(encode_uintvar(545), vec![0x84, 0x21]);
        assert_eq!(encode_uintvar(0), vec![0x00]);
        assert_eq!(decode_uintvar(&encode_uintvar(327680)).unwrap().0, 327680);
    }

    /// Build a synthetic WSP Connect mirroring the MXP600 capture (§7.4): WTP Invoke header
    /// `0x0b 13cc 12` (CON=0, Invoke, RID=1, TID 0x13cc, TCL=2), WSP Connect with Client-/Server-SDU
    /// = 327680.
    fn mxp600_connect() -> Vec<u8> {
        let sdu = encode_uintvar(327680);
        let cap = |id: u8| {
            let mut c = encode_uintvar((1 + sdu.len()) as u32);
            c.push(id);
            c.extend_from_slice(&sdu);
            c
        };
        let mut caps = cap(WSP_CAP_CLIENT_SDU);
        caps.extend_from_slice(&cap(WSP_CAP_SERVER_SDU));
        let mut wsp = vec![WSP_PDU_CONNECT, 0x10]; // Connect, version 0x10
        wsp.extend_from_slice(&encode_uintvar(caps.len() as u32)); // CapLen
        wsp.push(0x00); // HeadersLen 0
        wsp.extend_from_slice(&caps);
        let mut pkt = vec![0x0b, 0x13, 0xcc, 0x12]; // WTP Invoke header
        pkt.extend_from_slice(&wsp);
        pkt
    }

    #[test]
    fn classify_mxp600_connect() {
        assert_eq!(
            classify(&mxp600_connect()),
            WapRequest::WtpWspConnect {
                tid: 0x13cc,
                retransmission: true,
                client_sdu: 327680,
                server_sdu: 327680,
            }
        );
    }

    // §7.4 ConnectReply: WTP Result (TID 0x13cc -> 0x93cc) wrapping the WSP Connect-Reply with both
    // SDUs clamped 327680 -> 545.
    #[test]
    fn connect_reply_vector() {
        let wsp = wsp_connect_reply(327680, 327680);
        let out = wtp_result(0x13cc, &wsp);
        let want = hex("12 93 cc 02 01 08 00 03 80 84 21 03 81 84 21");
        assert_eq!(out, want);
    }

    // §7.4 WSP GET response prefix (TID 0x1234, XHTML content-type 0xC5).
    #[test]
    fn get_reply_prefix() {
        let wsp = wsp_reply(WSP_CT_XHTML, b"<html/>");
        let out = wtp_result(0x1234, &wsp);
        assert_eq!(&out[..7], &hex("12 92 34 04 20 01 c5")[..]);
        assert_eq!(&out[7..], b"<html/>");
    }

    // §7.4 WSP Resume (TID 0x13f5) -> [0x12, 0x93, 0xf5, 0x04, 0x20, 0x00].
    #[test]
    fn resume_ok_vector() {
        assert_eq!(wtp_result(0x13f5, &wsp_resume_ok()), hex("12 93 f5 04 20 00"));
    }

    // §7.4 control PDUs -> no datagram.
    #[test]
    fn control_pdus_no_response() {
        assert_eq!(classify(&hex("18 13 cc")), WapRequest::WtpControlNoResponse { pdu: 3 }); // ACK
        assert_eq!(
            classify(&hex("20 13 cc 01")),
            WapRequest::WtpControlNoResponse { pdu: 4 } // ABORT
        );
    }

    // §7.4 `[0x01,0x40,0x00]` (type nibble 0) -> text path -> Unsupported{3}.
    #[test]
    fn unsupported_three_bytes() {
        assert_eq!(classify(&hex("01 40 00")), WapRequest::Unsupported { len: 3 });
    }

    #[test]
    fn classify_wsp_get() {
        // WTP Invoke (CON=0, RID=0, TID 0x1234, TCL=2) + WSP GET "/status.xhtml".
        let path = b"/status.xhtml";
        let mut wsp = vec![WSP_PDU_GET];
        wsp.extend_from_slice(&encode_uintvar(path.len() as u32));
        wsp.extend_from_slice(path);
        let mut pkt = vec![0x08, 0x12, 0x34, 0x12];
        pkt.extend_from_slice(&wsp);
        assert_eq!(
            classify(&pkt),
            WapRequest::WtpWspGet {
                tid: 0x1234,
                retransmission: false,
                path: "/status.xhtml".to_string(),
            }
        );
    }

    #[test]
    fn classify_text_get() {
        assert_eq!(
            classify(b"GET /status.xhtml HTTP/1.0\r\n"),
            WapRequest::TextGet {
                path: "/status.xhtml".to_string()
            }
        );
    }

    // §7.4 endpoint swap: request id 0x2222 -> response id 0x2223; IPs/ports swapped; ttl 32.
    #[test]
    fn endpoint_swap() {
        let endpoint = WapEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let req = ip::build_ipv4_udp([10, 0, 0, 226], [10, 0, 0, 1], 49152, 9200, 0x2222, 32, b"x");
        let parsed = ip::parse_ipv4(&req).unwrap();
        let udp = ip::parse_udp(parsed.payload).unwrap();
        let resp = build_response_datagram(&endpoint, &parsed, udp.src_port, parsed.id, b"y");
        let rip = ip::parse_ipv4(&resp).unwrap();
        assert_eq!(rip.src, [10, 0, 0, 1]);
        assert_eq!(rip.dst, [10, 0, 0, 226]);
        assert_eq!(rip.id, 0x2223);
        assert_eq!(rip.ttl, 32);
        let rudp = ip::parse_udp(rip.payload).unwrap();
        assert_eq!(rudp.src_port, 9200);
        assert_eq!(rudp.dst_port, 49152);
    }
}
