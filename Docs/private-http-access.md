# Authenticated HTTP access on a trusted private network

HTTPS remains the default dashboard transport. An optional source-address allowlist can enable the existing login, management APIs and WebSocket over HTTP for clients on an explicitly allowed trusted LAN or private VPN. This option does not change certificate trust in a native app or disable HTTPS.

Updating through OTA does not enable this option automatically: the empty default allowlist keeps HTTP redirecting to HTTPS.

In the browser dashboard open **Config → Advanced → Raw config.toml**. Edit the existing `[dashboard]` section, preserving the station’s current credentials and other settings. After saving, use **Apply & Restart** to restart the station service; listener and allowlist changes do not take effect until that restart. For the FlowStation BS app using port 8080, use:

```toml
[dashboard]
port = 8080
https_port = 443
username = "operator"
password = "REPLACE_WITH_YOUR_OWN_PASSWORD"
http_allowed_networks = ["192.168.1.0/24", "10.42.0.0/24"]
```

Keep the station's existing account credentials, and replace the example subnet with the actual LAN and VPN client subnets. A single client can be restricted to an exact address, such as `10.42.0.11/32`. Private IPv4 networks, IPv6 ULA networks and explicit loopback addresses are supported. Public addresses, CGNAT ranges and hostnames are rejected.

After the configuration takes effect, in FlowStation BS enter the station's reachable LAN or VPN IP as **Host**, **8080** as **Port**, and disable **HTTPS**. Enter the existing dashboard username and password. For example, Host `192.168.1.100` on the LAN, or the station's private VPN IP remotely. The browser can continue using `https://<station-ip>/` on port **443**. Browser and app access are configuration choices: the HTTP listener does not identify an app by User-Agent and a browser on an allowed source network can also use HTTP. This configuration moves the primary HTTP listener from port 80 to port 8080, so port 80 no longer provides its previous redirect. A separate legacy 8080 redirect is not opened when 8080 is already the configured primary HTTP port. Set `https_port` explicitly as shown: it also prevents the old port-migration rule from treating port 8080 as a legacy configuration.

Only the direct TCP peer address is checked. `Forwarded`, `X-Forwarded-For`, the Host header and other client-supplied headers cannot grant HTTP access. Other peers continue to receive the canonical HTTPS redirect. An empty `http_allowed_networks` list preserves the default redirect behavior.

HTTP access always requires configured credentials and a valid dashboard session, including when the public overview is enabled. The login endpoint and WebSocket use the existing cookie protocol; the allowlist does not provide an alternative login method. Logout, expired sessions or runtime credential changes revoke a connected HTTP WebSocket. Allowlist and listener changes require a station service restart; runtime account changes do not.

HTTP itself does not encrypt credentials, session cookies or management data. On a LAN, credentials and session cookies travel in cleartext, so use only a network you trust. For remote access, carry HTTP through an encrypted VPN. Do not expose or forward this HTTP port to the public Internet. Continue using HTTPS for browser features that require a secure context, including the LST Dispatch microphone.

The standard and high-port presets in **System → Panel access** still select their existing port pairs. Selecting either preset replaces the custom `8080 / 443` pair above.
