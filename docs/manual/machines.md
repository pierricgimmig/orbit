<!--
Copyright (c) 2026 The Orbit Authors. All rights reserved.
Use of this source code is governed by a BSD-style license that can be
found in the LICENSE file.
-->

# Several machines

One viewer can show the processes of several machines on one timeline. The service whose page you open is the *hub*; every other machine's service links to it and streams its capture in. The hub puts the two clocks on one axis, files each remote's processes under its own machine, and a Record on the hub records on every linked machine at once.

## Link the machines

On the machine you will watch from, start the service as usual and open its page. On every other machine, start the service pointed at the hub:

```sh
orbit-service --join <hub-host>:44766
```

The remote dials the hub, so it works from a container, a CI runner or anything behind a firewall that allows nothing inbound. It still serves its own viewer on its own port.

The other way round, when the hub can reach the machines, is `--connect`:

```sh
orbit-service --connect render-01,build-mac:44766
```

or, with the hub already running, one request per machine:

```sh
curl -X POST http://127.0.0.1:44766/api/peers/connect -d '{"target": "render-01"}'
```

A target is `host`, `host:port` or a pasted `http://host:port/`; the port defaults to 44766. A link that drops is redialed every two seconds by whichever side dialed it. Up to 64 machines can be linked to one hub.

`GET /api/peers` lists the links: each machine's index, host name, core count, where its cores draw on the Scheduler track, and the state of its clock fit (below). `/api/status` reports how many machines are linked.

## What you see

The remotes' processes appear in the hub's process list as `name @host`, and on the timeline each machine has its own header in the rail, above its processes and threads, draggable and collapsible as a block. Pick any process from any machine and press Record: that machine captures the process, every other linked machine (the hub included) captures with no target, which still records its scheduler, its service and every process on it that instruments itself. Stop stops them all; Save writes one bundle with every machine in it.

A remote process's manual scopes, thread names and, when its service runs with the privilege for them, its scheduling slices, thread states and sample ticks all cross the link. Its cores are appended to the hub's Scheduler track as further lanes. What does not cross: hook lists, symbols, the sampling reports and the code views. Those run on the machine that owns the process, and the hub does not proxy them yet, so a function search or a symbol load on a remote process answers with an error. Choose hooks for a remote process by opening that machine's own viewer.

## Clocks

Every event is stamped on its own machine's capture clock, nanoseconds since that machine booted. The hub measures the offset between its clock and each remote's over the link itself: a burst of round-trip probes when the link comes up, then one a second. Each probe is the NTP exchange, four timestamps, whose error is at most half its round trip; the hub keeps the last two minutes of probes, keeps the ones whose round trip is within 1.5× the fastest, and fits offset and rate to those. Every event from that machine is moved onto the hub's clock as it arrives.

How well that works is a property of the network, and `GET /api/peers` says so for each link: `rtt_ns` is the best round trip in the window, so the alignment error is at most half of it; `offset_ns` and `rate_ppm` are the fit; `probes` is how many samples it rests on. On one machine, two services align within about 90 µs with a debug build; on a wired LAN expect a round trip of a few hundred microseconds and an alignment within tens; across a VPN or a WAN the band is milliseconds, enough to see two machines' load side by side, not enough to order events between them. Read the band before trusting a delta that spans machines. The viewer does not draw it yet.

The fit is applied once, as events arrive; a later probe does not move events already on screen, and the probes are not written into the bundle, so a saved capture cannot be re-fitted.

## Limits

- The API is as open as it always was: anything that can reach the hub's port can link a machine to it or start a capture, and anything that can reach a remote's port can link it as a hub. Keep the services on a network you trust, or bind them with `--host 127.0.0.1` and tunnel.
- Pids and thread ids of a remote are packed with its machine index in the top byte, so a remote id must fit in 24 bits. Every real process does; the agent's and the viewer's synthetic ids do not and are dropped from the link.
- One Scheduler track, with the remotes' cores appended; 255 lanes in all.
- No ssh transport and nothing is installed for you: a remote needs a running service and a port one side can reach.
