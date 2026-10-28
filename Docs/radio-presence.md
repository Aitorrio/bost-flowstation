# Radio presence on a TMO cell

A received, authorised `U-ITSI-DETACH` updates the local registration, dashboard and
Brew immediately. A radio that loses power or leaves coverage may send no detach
message. RF silence alone cannot distinguish that radio from a quiet listener.

The cell therefore uses individually addressed `D-LOCATION-UPDATE-COMMAND` checks:

- `cell_info.presence_check_secs` defaults to 30 seconds after the last accepted
  registration or decoded uplink. `0` disables these additional checks. Non-zero
  values are clamped to 5–3600 seconds.
- `cell_info.presence_response_secs` defaults to 30 seconds after MAC confirms that
  the command was transmitted. The allowed response grace is 10–300 seconds.
- `periodic_registration_secs` keeps its existing T351 policy, including explicit
  hour-long values. It is independent of the additional idle-radio checks.
- The radio remains registered during the response grace. A location update or
  subsequent valid decoded uplink confirms presence. No speech or PTT is required.
- An unanswered check marks current reachability unavailable in the local subscriber
  registry, dashboard and Brew. It retains the radio's known identity and group
  affiliations for coverage return and restart recovery. It does not delete identity
  history or saved positions. A later accepted registration restores reachability.
- A known but unavailable radio heard again on the uplink is immediately prompted to
  register, subject to the existing per-ISSI cooldown. Its stored groups and ciphering
  parameters are preserved. Uplink RSSI alone never creates a registration.

The default one-minute observation profile is:

```toml
[cell_info]
periodic_registration_secs = 3600
presence_check_secs = 30
presence_response_secs = 30
```

This aims for about one minute from the last confirmed radio activity to an
unavailable indication, plus scheduling delays. It does not promise instantaneous
RF-loss detection. A clean detach remains immediate. Shorter checks increase control
channel use and terminal battery consumption; validate the actual fleet before
changing a larger deployment.

Presence is tracked by ISSI. Multiple physical radios using the same ISSI cannot be
distinguished by this registry; a response from either confirms that ISSI's presence.

## Radio and station safeguards

Commands are limited to one per TDMA frame and observe known energy-economy wake
windows, with a bounded six-second fallback for a stale window. Independent checks
request no group report, so they do not make a radio discard existing affiliations.
They preserve the ciphering parameters agreed at registration.

Checks are deferred during authentication/OTAR and for individual calls, including
setup/ringing, as well as radios affiliated to an active group call. A receiver on a
traffic channel may not be monitoring MCCH even when it never transmits. The
observation window starts again after the call.

An offline/error/starting local RF path cannot prove that remote radios have left.
Checks pause while the cell is unavailable and start fresh windows when it returns.
A command still queued or discarded locally cannot mark a radio unavailable.
A locally untransmitted check older than fifteen seconds restarts the observation
window, allowing later probes instead of leaving a radio pending indefinitely.
Over-air transmission evidence survives LLC retry-state resets: a later locally
discarded retry cannot erase the fact that an earlier attempt reached MAC transmission.
A local processing gap of five seconds or more also cancels old challenges before
resuming observations. Startup recovery owns the radios it is actively restoring.

Implementation by Gustavo Encarnação — CS7ATL.
