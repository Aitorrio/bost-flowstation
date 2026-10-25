//! WAP status page renderer (WML2 / XHTML Mobile Profile).
//!
//! Two independent page families share this module:
//!
//!   * The **rich raw-UDP** page: [`render_wml2_status`] drives a Full -> Compact
//!     -> Text -> Tiny size ladder and is served over a plain UDP datagram (no
//!     WTP/WSP), budgeted to a single 576-octet N-PDU (548 bytes after IPv4+UDP).
//!   * The **WSP browser** pages: [`serve_wsp_path`] renders the bare
//!     `<html><body>` index and the paginated sector pages an Openwave
//!     UP.Browser fetches via WSP GET, hard-clamped to the browser's tiny SDU
//!     budgets (index 104, sector 144).
//!
//! Operator-supplied text is escaped and length-limited by
//! [`escape_xhtml_text_limited`] before it ever reaches the markup. The body is
//! byte-for-byte ASCII with no trailing newline.

/// Raw-UDP page cap: 576-octet one-slot N-PDU minus the 28-byte IPv4+UDP header.
pub const DEFAULT_WAP_STATUS_MAX_BYTES: usize = 548;

/// Openwave UP.Browser hard clamps, applied after the MTU budget.
pub const WSP_OPENWAVE_XHTML_INDEX_MAX: usize = 104;
pub const WSP_OPENWAVE_XHTML_SECTOR_MAX: usize = 144;
pub const WSP_OPENWAVE_WML_PAGE_MAX: usize = 144;

/// WSP content-type short-integers (never a textual MIME string).
pub const WSP_CT_WML: u8 = 0x88; // application/vnd.wap.wmlc
pub const WSP_CT_XHTML: u8 = 0xC5; // application/vnd.wap.xhtml+xml

// Escaped-byte caps per field (§4.4).
const CAP_TITLE: usize = 24;
const CAP_STATE: usize = 20;
const CAP_VERSION: usize = 32;
const CAP_LAST: usize = 32;
const CAP_HEALTH: usize = 32;
// Browser-page overrides (§4.4).
const CAP_BROWSER_INLINE: usize = 12;
const CAP_BROWSER_HEADING: usize = 14;

// Shared literals (§4.1). The two `<br>` spellings are deliberately distinct and
// must not be unified: the rich/raw-UDP family uses the spaced `<br />`, the WSP
// browser family uses the unspaced `<br/>`.
const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const XHTML_MP_DOCTYPE: &str = "<!DOCTYPE html PUBLIC \"-//WAPFORUM//DTD XHTML Mobile 1.0//EN\" \"http://www.wapforum.org/DTD/xhtml-mobile10.dtd\">";
const XHTML_HTML_OPEN: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\">";
const TINY_XHTML_PREFIX: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>";
const TINY_XHTML_SUFFIX: &str = "</body></html>";
const TINY_XHTML_BR: &str = "<br />"; // rich / raw-UDP family (spaced)
const BROWSER_BR: &str = "<br/>"; // WSP browser family (unspaced)

/// A point-in-time view of the cell the status page reflects.
#[derive(Debug, Clone, Default)]
pub struct WapStatusSnapshot {
    pub title: String,
    pub version: String,
    pub state: String,
    pub registered_ms: u32,
    pub group_count: u32,
    pub private_count: u32,
    pub sds_count: u32,
    /// One line per currently-registered radio. When non-empty its length, not
    /// `registered_ms`, is the MS count shown on the page (§4.6).
    pub radio_lines: Vec<String>,
    pub uptime_secs: u64,
    pub last_activity: String,
    pub health: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderError {
    /// Title is the one hard precondition; an empty title is never rendered.
    EmptyTitle,
    /// Not even the Tiny page fits `max`; `len` is Tiny's rendered length.
    RenderedTooLarge { len: usize, max: usize },
}

/// Escape-then-truncate on the **escaped** byte length (§4.3).
///
/// For each character the escaped fragment is computed; if appending it would
/// exceed `max` the result is marked truncated and the loop breaks (an entity is
/// never split). After the loop, if the result was truncated and one more byte
/// still fits, a `~` (0x7E) is appended to signal elision.
pub fn escape_xhtml_text_limited(input: &str, max: usize) -> String {
    let mut escaped = String::new();
    let mut truncated = false;
    for ch in input.chars() {
        let fragment: &str = match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            '\n' | '\r' | '\t' => " ",
            c if (c as u32) < 0x20 => "?",
            _ => {
                // Ordinary character: measure its UTF-8 byte length in place.
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                if escaped.len() + s.len() > max {
                    truncated = true;
                    break;
                }
                escaped.push_str(s);
                continue;
            }
        };
        if escaped.len() + fragment.len() > max {
            truncated = true;
            break;
        }
        escaped.push_str(fragment);
    }
    if truncated && escaped.len() + 1 <= max {
        escaped.push('~');
    }
    escaped
}

// ---- field compactors (§4.6) -------------------------------------------------

/// Two-field, zero-padded uptime, e.g. `93784 -> "1d02h"`.
pub fn compact_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{}d{:02}h", d, h)
    } else if h > 0 {
        format!("{}h{:02}m", h, m)
    } else {
        format!("{}m{:02}s", m, s)
    }
}

/// Four-field unpadded uptime, days clamped to 99, e.g. `93784 -> "1d2h3m4s"`.
pub fn compact_tiny_uptime(secs: u64) -> String {
    let d = (secs / 86_400).min(99);
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    format!("{}d{}h{}m{}s", d, h, m, s)
}

/// Browser uptime; drops the minute field when zero.
pub fn compact_browser_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if m == 0 {
        format!("{}d{}h", d, h)
    } else {
        format!("{}d{}h{}m", d, h, m)
    }
}

/// Collapse a verbose state string to a three-way tiny token.
pub fn compact_tiny_state(state: &str) -> &'static str {
    if state.contains("CRITICAL") {
        "Err"
    } else if state.contains("DEGRADED") {
        "Warn"
    } else {
        "OK"
    }
}

/// Reduce a dotted version to its numeric core, e.g. `v0.1.69_dev-test -> 0.1.69`.
pub fn compact_version(version: &str) -> String {
    let bytes = version.as_bytes();
    let start = bytes.iter().position(|b| b.is_ascii_digit());
    match start {
        None => version.to_string(),
        Some(i) => {
            let core: String = version[i..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            core.trim_end_matches('.').to_string()
        }
    }
}

/// The MS count shown on the page: the radio-line count when any are present,
/// otherwise the plain `registered_ms` figure (§4.6).
pub fn rendered_registered_ms(snap: &WapStatusSnapshot) -> u32 {
    if snap.radio_lines.is_empty() {
        snap.registered_ms
    } else {
        snap.radio_lines.len() as u32
    }
}

// ---- rich raw-UDP ladder (§4.2) ---------------------------------------------

/// Render the rich raw-UDP status page, picking the first ladder mode that fits
/// `max_bytes`: Full -> Compact -> Text -> Tiny.
pub fn render_wml2_status(snap: &WapStatusSnapshot, max_bytes: usize) -> Result<Vec<u8>, RenderError> {
    if snap.title.is_empty() {
        return Err(RenderError::EmptyTitle);
    }

    let full = render_full(snap);
    if full.len() <= max_bytes {
        return Ok(full.into_bytes());
    }
    let compact = render_compact(snap);
    if compact.len() <= max_bytes {
        return Ok(compact.into_bytes());
    }
    let text = render_text(snap);
    if text.len() <= max_bytes {
        return Ok(text.into_bytes());
    }
    let tiny = render_tiny(snap, max_bytes);
    if tiny.len() <= max_bytes {
        return Ok(tiny.into_bytes());
    }
    Err(RenderError::RenderedTooLarge {
        len: tiny.len(),
        max: max_bytes,
    })
}

fn render_full(snap: &WapStatusSnapshot) -> String {
    let title = escape_xhtml_text_limited(&snap.title, CAP_TITLE);
    let state = escape_xhtml_text_limited(&snap.state, CAP_STATE);
    let version = escape_xhtml_text_limited(&snap.version, CAP_VERSION);
    let last = escape_xhtml_text_limited(&snap.last_activity, CAP_LAST);
    let health = escape_xhtml_text_limited(&snap.health, CAP_HEALTH);
    let ms = rendered_registered_ms(snap);

    let mut p = String::new();
    p.push_str(XML_DECL);
    p.push('\n');
    p.push_str(XHTML_MP_DOCTYPE);
    p.push('\n');
    p.push_str(XHTML_HTML_OPEN);
    p.push_str("<head><title>");
    p.push_str(&title);
    p.push_str("</title>");
    p.push_str("<meta http-equiv=\"Cache-Control\" content=\"no-cache\" />");
    p.push_str("<meta http-equiv=\"refresh\" content=\"8;url=/\" />");
    p.push_str("<style>body{font-family:sans-serif;margin:4px}.k{font-weight:bold}.hero{padding:2px}</style>");
    p.push_str("</head><body>");
    p.push_str("<div class=\"hero\"><h1>");
    p.push_str(&title);
    p.push_str("</h1><p class=\"sub\">WAP 2.0 / WML2 live core</p>");
    p.push_str("<p class=\"st\">");
    p.push_str(&state);
    p.push_str("</p><p class=\"cnt\">");
    p.push_str(&format!(
        "<span class=\"k\">MS</span> {} <span class=\"k\">G</span> {} <span class=\"k\">P</span> {} <span class=\"k\">SDS</span> {}",
        ms, snap.group_count, snap.private_count, snap.sds_count
    ));
    p.push_str("</p><p><span class=\"k\">Up</span> ");
    p.push_str(&compact_uptime(snap.uptime_secs));
    p.push_str(" <span class=\"k\">Ver</span> ");
    p.push_str(&version);
    p.push_str("</p><p>Last: ");
    p.push_str(&last);
    p.push_str("</p><p>Health:");
    p.push_str(&health);
    p.push_str("</p></div></body></html>");
    p
}

fn render_compact(snap: &WapStatusSnapshot) -> String {
    let title = escape_xhtml_text_limited(&snap.title, CAP_TITLE);
    let state = escape_xhtml_text_limited(&snap.state, CAP_STATE);
    let version = escape_xhtml_text_limited(&snap.version, CAP_VERSION);
    let last = escape_xhtml_text_limited(&snap.last_activity, CAP_LAST);
    let health = escape_xhtml_text_limited(&snap.health, CAP_HEALTH);
    let ms = rendered_registered_ms(snap);

    let mut p = String::new();
    p.push_str(XML_DECL);
    p.push('\n');
    p.push_str(XHTML_MP_DOCTYPE);
    p.push('\n');
    p.push_str(XHTML_HTML_OPEN);
    p.push_str("<head><title>");
    p.push_str(&title);
    p.push_str("</title>");
    p.push_str("<meta http-equiv=\"refresh\" content=\"10;url=/\" />");
    p.push_str("<style>.k{font-weight:bold}</style>");
    p.push_str("</head><body>");
    p.push_str("<p>");
    p.push_str(&state);
    p.push_str("</p><p>");
    p.push_str(&format!(
        "MS:{} G:{} P:{} SDS:{}",
        ms, snap.group_count, snap.private_count, snap.sds_count
    ));
    p.push_str("</p><p>Up:");
    p.push_str(&compact_uptime(snap.uptime_secs));
    p.push_str(" Ver:");
    p.push_str(&version);
    p.push_str("</p><p>Last:");
    p.push_str(&last);
    p.push_str("</p><p>Health:");
    p.push_str(&health);
    p.push_str("</p></body></html>");
    p
}

fn render_text(snap: &WapStatusSnapshot) -> String {
    let title = escape_xhtml_text_limited(&snap.title, CAP_TITLE);
    let state = escape_xhtml_text_limited(&snap.state, CAP_STATE);
    let version = escape_xhtml_text_limited(&snap.version, CAP_VERSION);
    let last = escape_xhtml_text_limited(&snap.last_activity, CAP_LAST);
    let health = escape_xhtml_text_limited(&snap.health, CAP_HEALTH);
    let ms = rendered_registered_ms(snap);

    let lines = [
        format!("{}: {}", title, state),
        format!("MS {} G {} P {} SDS {}", ms, snap.group_count, snap.private_count, snap.sds_count),
        format!("Up {} Ver {}", compact_uptime(snap.uptime_secs), version),
        format!("Last: {}", last),
        format!("Health:{}", health),
    ];

    let mut p = String::new();
    p.push_str(XHTML_HTML_OPEN);
    p.push_str("<head><title>");
    p.push_str(&title);
    p.push_str("</title>");
    p.push_str("<meta http-equiv=\"refresh\" content=\"10;url=/\" />");
    p.push_str("</head><body>");
    p.push_str(&lines.join(TINY_XHTML_BR));
    p.push_str("</body></html>");
    p
}

fn render_tiny(snap: &WapStatusSnapshot, max_bytes: usize) -> String {
    let title = escape_xhtml_text_limited(&snap.title, CAP_TITLE);
    let last = escape_xhtml_text_limited(&snap.last_activity, CAP_LAST);
    let ms = rendered_registered_ms(snap);

    // Candidate lines in priority order; the first is always kept, the rest are
    // appended only while they fit (counts and the last-activity line are the
    // first to be dropped on a tight budget).
    let base = format!("{}: {}", title, compact_tiny_state(&snap.state));
    let optional = [
        format!("Version: {}", compact_version(&snap.version)),
        format!("Uptime {}", compact_tiny_uptime(snap.uptime_secs)),
        format!("MS {} G {} P {} SDS {}", ms, snap.group_count, snap.private_count, snap.sds_count),
        format!("Last {}", last),
    ];

    let frame = TINY_XHTML_PREFIX.len() + TINY_XHTML_SUFFIX.len();
    let mut body = base;
    for line in &optional {
        let candidate_len = body.len() + TINY_XHTML_BR.len() + line.len();
        if frame + candidate_len <= max_bytes {
            body.push_str(TINY_XHTML_BR);
            body.push_str(line);
        }
    }

    let mut p = String::with_capacity(frame + body.len());
    p.push_str(TINY_XHTML_PREFIX);
    p.push_str(&body);
    p.push_str(TINY_XHTML_SUFFIX);
    p
}

// ---- WSP browser family (§4.5, §7.4) ----------------------------------------

/// Serve a WSP browser GET: map the request path to a content-type short-integer
/// and a rendered body, hard-clamped to the Openwave browser budgets.
pub fn serve_wsp_path(path: &str, snap: &WapStatusSnapshot, max_bytes: usize) -> (u8, Vec<u8>) {
    let is_wml = path.contains(".wml");
    let sector = parse_sector(path);
    if is_wml {
        let cap = max_bytes.min(WSP_OPENWAVE_WML_PAGE_MAX);
        (WSP_CT_WML, browser_wml_sector(snap, sector.unwrap_or(0), cap).into_bytes())
    } else if let Some(s) = sector {
        let cap = max_bytes.min(WSP_OPENWAVE_XHTML_SECTOR_MAX);
        (WSP_CT_XHTML, browser_xhtml_sector(snap, s, cap).into_bytes())
    } else {
        let cap = max_bytes.min(WSP_OPENWAVE_XHTML_INDEX_MAX);
        (WSP_CT_XHTML, browser_xhtml_index(snap, cap).into_bytes())
    }
}

/// Extract the `s=` sector query parameter, if present.
fn parse_sector(path: &str) -> Option<usize> {
    let q = path.split('?').nth(1)?;
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix("s=") {
            return v.parse::<usize>().ok();
        }
    }
    None
}

/// Sector table: [Summary, Health, Radios chunks (5/page), Activity].
fn sector_labels(snap: &WapStatusSnapshot) -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = Vec::new();
    v.push(("Summary", format!("State {}", compact_tiny_state(&snap.state))));
    v.push(("Health", format!("Health {}", escape_xhtml_text_limited(&snap.health, CAP_HEALTH))));
    for chunk in snap.radio_lines.chunks(5) {
        let ids: Vec<String> = chunk
            .iter()
            .map(|l| escape_xhtml_text_limited(l, CAP_BROWSER_HEADING))
            .collect();
        v.push(("Radios", ids.join(" ")));
    }
    if !snap.last_activity.is_empty() {
        v.push(("Activity", escape_xhtml_text_limited(&snap.last_activity, CAP_LAST)));
    }
    v
}

fn browser_xhtml_index(snap: &WapStatusSnapshot, _cap: usize) -> String {
    let title = escape_xhtml_text_limited(&snap.title, CAP_BROWSER_INLINE);
    let mut p = String::new();
    p.push_str("<html><body>Welcome to ");
    p.push_str(&title);
    p.push('!');
    p.push_str(BROWSER_BR);
    p.push_str("<a href=\"/status.wml?s=1\">Status</a>");
    p.push_str("</body></html>");
    p
}

fn browser_xhtml_sector(snap: &WapStatusSnapshot, sector: usize, _cap: usize) -> String {
    let sectors = sector_labels(snap);
    let count = sectors.len().max(1);
    let idx = sector.min(count - 1);
    let (label, body) = sectors.get(idx).cloned().unwrap_or(("Summary", String::new()));
    let prev = idx.saturating_sub(1);
    let next = (idx + 1).min(count - 1);

    let mut p = String::new();
    p.push_str("<html><body>");
    p.push_str(label);
    p.push(' ');
    p.push_str(&format!("{}/{}", idx + 1, count));
    p.push_str(BROWSER_BR);
    p.push_str(&body);
    p.push_str(BROWSER_BR);
    p.push_str(&format!("<a href=\"?s={}\">P</a> ", prev));
    p.push_str(&format!("<a href=\"?s={}\">N</a> ", next));
    p.push_str("<a href=\"/\">H</a>");
    p.push_str("</body></html>");
    p
}

fn browser_wml_sector(snap: &WapStatusSnapshot, sector: usize, _cap: usize) -> String {
    let sectors = sector_labels(snap);
    let count = sectors.len().max(1);
    let idx = sector.min(count - 1);
    let (label, body) = sectors.get(idx).cloned().unwrap_or(("Summary", String::new()));
    let prev = idx.saturating_sub(1);

    let mut p = String::new();
    p.push_str("<wml><card><p>");
    p.push_str(label);
    p.push(' ');
    p.push_str(&body);
    p.push_str("</p><p>");
    p.push_str(&format!("<a href=\"?s={}\">P</a> ", prev));
    p.push_str("<a href=\"/\">H</a>");
    p.push_str("</p></card></wml>");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            version: "v0.1.69_dev-test".to_string(),
            state: "ON AIR".to_string(),
            registered_ms: 3,
            group_count: 0,
            private_count: 0,
            sds_count: 0,
            radio_lines: vec!["MS 2260082 -38dB G1 SA".to_string(), "MS 2260618 -41dB G1".to_string()],
            uptime_secs: 93_784,
            last_activity: "SDS 2260082>2260618".to_string(),
            health: "OK".to_string(),
        }
    }

    // ---- §4.3 escape-then-truncate vectors ----

    #[test]
    fn escape_vectors() {
        assert_eq!(escape_xhtml_text_limited("&&&&", 10), "&amp;&amp;");
        assert_eq!(escape_xhtml_text_limited("&&&&", 11), "&amp;&amp;~");
        assert_eq!(escape_xhtml_text_limited("<tag>", 8), "&lt;tag~");
        assert_eq!(escape_xhtml_text_limited("abc", 0), "");
    }

    #[test]
    fn escape_never_splits_entity() {
        // 12 bytes allows two full &amp; (10) but not a third; truncated, +~ fits.
        assert_eq!(escape_xhtml_text_limited("&&&", 12), "&amp;&amp;~");
    }

    #[test]
    fn escape_control_and_whitespace() {
        assert_eq!(escape_xhtml_text_limited("a\tb\nc", 16), "a b c");
        assert_eq!(escape_xhtml_text_limited("x\u{0001}y", 16), "x?y");
        assert_eq!(escape_xhtml_text_limited("he said \"hi\"", 64), "he said &quot;hi&quot;");
        assert_eq!(escape_xhtml_text_limited("it's", 64), "it&apos;s");
    }

    // ---- compactors ----

    #[test]
    fn uptime_formatters() {
        assert_eq!(compact_uptime(93_784), "1d02h");
        assert_eq!(compact_tiny_uptime(93_784), "1d2h3m4s");
    }

    #[test]
    fn tiny_uptime_clamps_days() {
        assert_eq!(compact_tiny_uptime(200 * 86_400 + 5), "99d0h0m5s");
    }

    #[test]
    fn state_and_version_compactors() {
        assert_eq!(compact_tiny_state("ON AIR"), "OK");
        assert_eq!(compact_tiny_state("CELL CRITICAL"), "Err");
        assert_eq!(compact_tiny_state("DEGRADED link"), "Warn");
        assert_eq!(compact_version("v0.1.69_dev-test"), "0.1.69");
        assert_eq!(compact_version("1.2.3"), "1.2.3");
    }

    #[test]
    fn rendered_ms_prefers_radio_lines() {
        let snap = sample();
        assert_eq!(rendered_registered_ms(&snap), 2); // 2 radio lines, not registered_ms=3
        let mut no_radios = snap.clone();
        no_radios.radio_lines.clear();
        assert_eq!(rendered_registered_ms(&no_radios), 3);
    }

    // ---- §7.5 rich page vectors ----

    #[test]
    fn full_page_contains_required_fragments() {
        let page = render_wml2_status(&sample(), 2048).unwrap();
        let s = String::from_utf8(page).unwrap();
        assert!(s.contains("<span class=\"k\">MS</span> 2"), "MS count quirk");
        assert!(s.contains("Up</span> 1d02h"), "compact uptime");
        assert!(s.contains("Ver</span> v0.1.69_dev-test"), "version uncompacted");
        assert!(s.contains("Last: SDS 2260082&gt;2260618"), "escaped last activity");
        assert!(s.contains("Health:OK"), "health no-space");
        assert!(s.starts_with(XML_DECL), "full page leads with the xml decl");
        assert!(s.contains(XHTML_MP_DOCTYPE), "full page carries the XHTML-MP doctype");
        assert!(!s.ends_with('\n'), "no trailing newline");
    }

    #[test]
    fn tiny_page_vectors() {
        let page = render_wml2_status(&sample(), 128).unwrap();
        let s = String::from_utf8(page).unwrap();
        assert!(s.len() <= 128);
        assert!(s.contains("Nexus-BS: OK"));
        assert!(s.contains("Version: 0.1.69"));
        assert!(s.contains("Uptime 1d2h3m4s"));
        assert_eq!(s.matches("<br />").count(), 2, "exactly two spaced <br />");
        assert!(s.starts_with(TINY_XHTML_PREFIX));
        assert!(s.ends_with(TINY_XHTML_SUFFIX));
    }

    #[test]
    fn empty_title_is_rejected() {
        let mut snap = sample();
        snap.title.clear();
        assert_eq!(render_wml2_status(&snap, 2048), Err(RenderError::EmptyTitle));
    }

    #[test]
    fn too_small_reports_tiny_length() {
        let snap = sample();
        match render_wml2_status(&snap, 10) {
            Err(RenderError::RenderedTooLarge { len, max }) => {
                assert_eq!(max, 10);
                assert!(len > 10);
            }
            other => panic!("expected RenderedTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn ladder_descends_with_budget() {
        let snap = sample();
        // A generous budget keeps the Full page (leads with the xml decl).
        let big = String::from_utf8(render_wml2_status(&snap, 2048).unwrap()).unwrap();
        assert!(big.starts_with(XML_DECL));
        // The tightest budget that still renders drops to the Tiny family.
        let tiny = String::from_utf8(render_wml2_status(&snap, 128).unwrap()).unwrap();
        assert!(tiny.starts_with(TINY_XHTML_PREFIX));
    }

    // ---- §7.4 WSP browser vectors ----

    #[test]
    fn wsp_xhtml_index() {
        let (ct, body) = serve_wsp_path("/status.xhtml", &sample(), 541);
        assert_eq!(ct, WSP_CT_XHTML); // payload[6] after WTP+WSP framing
        assert!(body.len() <= WSP_OPENWAVE_XHTML_INDEX_MAX, "index clamped to 104");
        let s = String::from_utf8(body).unwrap();
        assert!(s.contains("Welcome to Nexus-BS!"));
        assert!(s.contains("href=\"/status.wml?s=1\""));
    }

    #[test]
    fn wsp_xhtml_sector_health() {
        let (ct, body) = serve_wsp_path("/status.xhtml?s=1", &sample(), 541);
        assert_eq!(ct, WSP_CT_XHTML);
        assert!(body.len() <= WSP_OPENWAVE_XHTML_SECTOR_MAX, "sector clamped to 144");
        let s = String::from_utf8(body).unwrap();
        assert!(s.contains("Health OK"));
        assert!(s.contains("href=\"?s=0\""));
        assert!(s.contains("href=\"/\""));
    }

    #[test]
    fn wsp_wml_sector() {
        let (ct, body) = serve_wsp_path("/status.wml?s=1", &sample(), 541);
        assert_eq!(ct, WSP_CT_WML);
        assert!(body.len() <= WSP_OPENWAVE_WML_PAGE_MAX);
        let s = String::from_utf8(body).unwrap();
        assert!(s.starts_with("<wml><card><p>"));
        assert!(s.contains("Health OK"));
    }

    #[test]
    fn wsp_sector_index_clamps_to_last() {
        // An out-of-range sector request clamps to the last available page.
        let (_ct, body) = serve_wsp_path("/status.xhtml?s=99", &sample(), 541);
        let s = String::from_utf8(body).unwrap();
        assert!(s.contains("href=\"/\""));
    }
}
