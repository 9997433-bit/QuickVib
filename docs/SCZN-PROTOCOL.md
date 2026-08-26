# SCZN — the M300's TCP transfer protocol

> **Status: transcribed from the vendor specification `通讯协议.md` (传输协议设计) and implemented
> in `crates/quickvib-sczn`. Not yet confirmed against real hardware.** Three readings in §4 and
> §7 are inferences rather than statements, because the specification does not say; each is
> marked **bench** and each is the sort of thing that produces a link which connects and then
> does nothing. The sibling UDP configuration protocol (`网络配置协议` /
> `net_conf_protocol.md`) is summarised in §10 and is not implemented.

This is the protocol the **device** speaks. It is not the protocol QuickVib's `--backend tcp`
speaks, and the difference is the whole reason this document exists:

| Path | Wire | Who binds the socket | Simulator |
| --- | --- | --- | --- |
| `--backend tcp` | bare little-endian `f32`, no framing at all | QuickVib | `m300-sim` |
| `--backend m300` | SCZN frames, this document | the vendor SDK (`docs/M300-NATIVE.md` §6) | `m300-device-sim` |

With `--backend m300`, QuickVib never sees an SCZN byte: `m300_sdk.dll` binds the port, parses
the frames and hands decoded blocks to a callback. So the only thing this protocol is *for*, on
our side, is being on the other end of it — which is what `m300-device-sim` is.

## 1. Transport and roles

| | |
| --- | --- |
| Transport | TCP |
| Server | the **host** (PC / QuickVib / the SDK) |
| Client | the **device** (the vibrometer) |
| Default port | `9123` |
| Reconnection | the device dials on a timer until it succeeds |

The direction is worth restating because it inverts the usual expectation: the instrument is the
one that dials out. A simulator for it is therefore a TCP *client* with a reconnect loop, not a
listener.

## 2. Frame layout

```text
0      4    5    6        8              12                12+N
+------+----+----+--------+--------------+------ ... ------+--------+
| SCZN | ver| cmd| cmd_id | data_length  |     payload     |  crc32 |
+------+----+----+--------+--------------+------ ... ------+--------+
   4     1    1      2           4              N              4
```

| Field | Size | Notes |
| --- | --- | --- |
| magic | 4 | `"SCZN"` = `0x53435A4E` |
| version | 1 | `0x00` is the only version defined |
| command | 1 | §3 |
| command id | 2 | the host's correlation id; **the device echoes it unchanged** |
| data length | 4 | payload bytes, not counting header or trailer |
| payload | N | command-specific |
| crc32 | 4 | over magic … payload inclusive; §7 |

**Byte order: big-endian, for everything outside the payload.** The specification never says so
in prose. What it does say is `struct.pack(">4s B B H I", …)` in its worked Python example, and
it quotes the magic as the *number* `0x53435A4E`, which is `"SCZN"` read big-endian. Those two
agree, and the example's printed bytes confirm them:

```text
53435A4E 00 01 0001 00000000 A57105A2
  SCZN   v  cmd cmd_id  len     crc
```

Note that the sibling UDP protocol is explicitly **little**-endian and has a different header
(it carries a MAC address), so the two do not constrain each other and neither is a typo.

## 3. Command table

Device-to-host commands are marked ←; host-to-device →.

| Code | Direction | Meaning | `m300-device-sim` |
| --- | --- | --- | --- |
| `0x00` | → | Start acquisition | replies `0x01` OK, begins streaming |
| `0x01` | ← | Reply to start | |
| `0x02` | → | Stop acquisition | replies `0x03` OK, stops immediately |
| `0x03` | ← | Reply to stop | |
| `0x04` | ← | Data upload | §4 |
| `0x06` | → | Write parameters | replies `0x07`; all-or-nothing |
| `0x07` | ← | Reply to write | |
| `0x08` | → | Read parameters | replies `0x09` with stored values |
| `0x09` | ← | Reply to read | |
| `0x0A` | → | List parameter ids | replies `0x0B` |
| `0x0B` | ← | Reply to list | |
| `0x0C` | → | Read device status | replies `0x0D` |
| `0x0D` | ← | Reply to status | |
| `0x0E` | → | Remove DC | replies `0x0F` OK |
| `0x0F` | ← | Reply to DC removal | |
| `0xD0`/`0xD2` | → | Start / stop pulse output | replies `0xD1`/`0xD3` **fail** — no pulse generator behind a simulator |
| `0xD1`/`0xD3` | ← | Replies to the above | |
| `0xF8` | → | Begin firmware upgrade | replies `0xF9` **fail** |
| `0xF9` | ← | Reply to begin | |
| `0xFA` | ← | Upgrade request | never sent |
| `0xFB` | → | Firmware parameters (encrypted CRC + size) | replies `0xFC` **fail** |
| `0xFC` | ← | Reply to parameters | |
| `0xFD` | → | Firmware chunk (≤ 1280 bytes) | replies `0xFE` **fail** |
| `0xFE` | ← | Reply to chunk | |
| `0xFF` | ← | Upgrade result | never sent |

Anything else is ignored: the protocol has no generic negative acknowledgement, so a device that
answered an unrecognised command would have to invent a reply code, and a host that received one
would have no way to interpret it. Silence is what the specification implies and what real
firmware does.

**Result field.** Every `*_REPLY` above carries a two-byte result: `0x0000` success, `0x0001`
failure, everything else "待定" (to be decided). The upgrade family is refused rather than
ignored precisely so the host's own error handling runs — refusing in the documented shape keeps
the link usable, which is the interesting path to exercise.

### Common payload shapes

Every count, id and length below is a big-endian 32-bit word.

| Command | Payload |
| --- | --- |
| `0x06` write | `count, [id, length, value]…` |
| `0x07` reply | `result` |
| `0x08` read | `count, [id]…` |
| `0x09` reply | `result, count, [id, length, value]…` |
| `0x0A` list | *(empty)* |
| `0x0B` reply | `result, count, [id]…` |
| `0x0C` status | `count, [id]…` |
| `0x0D` reply | `result, count, [id, value]…` — **no length field**; see §6 |

## 4. Data upload (`0x04`)

```text
0            4              8                    8 + 4·N
+------------+--------------+------- ... --------+
| data_type  | point_count  |  N × f32 samples   |
+------------+--------------+------- ... --------+
```

`data_type` repeats the `0x00000001` parameter code (velocity / displacement / acceleration /
I-Q). `point_count` is the number of samples in **one channel**.

**Samples are little-endian IEEE-754 `f32`.** This is not from the SCZN specification — it is
from the SDK, which documents the callback's buffer as `point_count × 4` contiguous
little-endian `f32` and cross-checks `data_len == point_count * 4` before reading it
(`docs/M300-NATIVE.md` §6, §7). Since the SDK passes the payload bytes straight through, that is
what the wire must carry.

**bench — the two prefix words.** Nothing states their byte order. Big-endian is the consistent
reading (the header is big-endian and the prefix is not sample data), and it is what
`m300-device-sim` sends by default, but it means one frame carries both orders. `--prefix-endian
little` flips it so a bench run can settle the question in one attempt instead of a rebuild.
Getting this wrong is cheap to detect: `point_count` read the wrong way round is a number in the
millions, and the SDK's own `data_len` check rejects the block.

For `data_type = 0x03` each 4-byte word is two 16-bit lanes, I then Q, rather than an `f32`.
`m300-device-sim` will name that type but does not synthesize it: inventing plausible quadrature
data would be a fiction about the instrument rather than about the wire.

## 5. Parameters

Ids are a flat 32-bit space. Widths matter — status replies (§6) have no length field.

| Id | Width | Meaning |
| --- | --- | --- |
| `0x00000000` | 1 | Sample rate (code, §5.1) |
| `0x00000001` | 1 | Upload data type: `0` velocity, `1` displacement, `2` acceleration, `3` I/Q |
| `0x00000002` | 1 | Indicator-laser level, 0–10 |
| `0x00000003` | 1 | Signal strength |
| `0x00000004` | 41 or 45 | Hardware information (§5.2) |
| `0x00000005` | 1 | Low-pass filter band (code) |
| `0x00000006` | 1 | High-pass filter band, 档位 1–10 |
| `0x00000007` | 1 | Velocity range, `0x00` ±2.45 μm/s … `0x10` ±7 m/s |
| `0x00000008` | 1 | Displacement range, `0x00` ±0.245 μm … `0x12` ±0.5 m |
| `0x00000009` | 1 | Acceleration range, `0x00` ±1.225 m/s² … `0x0B` ±612500 m/s² |
| `0x0000000A`–`0x0000000D` | 1 | Analogue output type and switch, ports 1 and 2 |
| `0x0000000E` | 1 | Trigger type: `0` free, `1` software, `2` hardware, `3` custom |
| `0x0000000F`–`0x00000011` | 1 | Trigger edge, level, channel |
| `0x00000012` | 4 | Triggered-capture length |
| `0x00000013` | 4 | Pre-trigger samples |
| `0x00000014`–`0x00000019` | 4 | Laser current, TEC target, digital range, P, I, D |
| `0x10000000`–`0x1000000D` | 1 or 4 | Front-end filter, DC removal, I/Q correction, per-unit amplitude trims |

`m300-device-sim` carries fourteen of these — the ones a host actually configures — and refuses
a write to any other id, because a device's parameter table is fixed in firmware and a host
cannot add a row to it. A write batch is all-or-nothing: a half-applied configuration is the one
outcome a caller cannot recover from.

### 5.1 Code ladders (`device_type = 0x01`, M300)

Every knob is an **index into a vendor table**, never a quantity. `0x05` means 100 kHz, not 5 Hz.

Sample rate (`0x00000000`): 2 k, 5 k, 10 k, 20 k, 50 k, 100 k, 200 k, 400 k, 800 k, 1 M, 2 M,
4 M, 5 M, 10 M, 20 M Hz for codes `0x00`–`0x0E`.

Low-pass (`0x00000005`): 100, 500, 1 k, 2 k, 5 k, 10 k, 20 k, 40 k, 80 k, 100 k, 160 k, 320 k,
500 k, 1 M, 3 M Hz for codes `0x00`–`0x0E`.

These are the same two ladders `crates/quickvib-m300/src/maps.rs` carries for the SDK backend;
`crates/quickvib-sczn/src/params.rs` mirrors them so the simulator does not have to depend on a
Windows-only crate. **The vendor repeats in every document that the low-pass band must be set
together with the sample rate at a matching band**, or "data may be abnormal or the device may
refuse"; `m300-device-sim` pairs them automatically, rounding the band down when there is no
exact match (there is no 50 kHz or 200 kHz filter).

The specification also defines a `device_type = 0x02` ladder for a second, unnamed model. It is
transcribed in the vendor document and deliberately **not** implemented here: nothing in QuickVib
targets that device, and a code table nobody exercises is a table that silently rots.

### 5.2 Hardware information (`0x00000004`)

41 bytes for the M300; the 45-byte variant inserts a 4-byte FPGA version after the serial.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | Device type — `0x01` M300, `0x02` the second model |
| 1 | 4 | IP address |
| 5 | 4 | Subnet mask |
| 9 | 4 | Default gateway |
| 13 | 6 | MAC address |
| 19 | 10 | Serial number, ASCII, **may not be NUL-terminated** |
| *(45-byte form only)* | 4 | FPGA version |
| 29 | 3 | Bootloader version `[major, minor, patch]` |
| 32 | 3 | Application version |
| 35 | 4 | Server IP |
| 39 | 2 | Server port |

**bench — the server port's byte order.** Addresses are network byte order, which for four
octets means "as written". The port is the odd one out: this protocol is big-endian throughout,
but the *UDP* protocol states the opposite for its own port fields. `m300-device-sim` writes it
big-endian, for consistency with the header.

## 6. Device status (`0x0C` / `0x0D`)

| Id | Width | Meaning |
| --- | --- | --- |
| `0x00000000` | 1 | Running state: `0` idle, `1` acquiring, `2` upgrading, `3` fault |
| `0x00000001` | 4 | TEC NTTC, `i32` ohms |
| `0x00000002` | 4 | Board temperature, `f32` °C |
| `0x00000003` | 4 | Photodiode current, `f32` mA |
| `0x00000004` | 4 | Signal strength: I in the low half, Q in the high half |

Status entries carry **no length field** — the reply is `id, value, id, value` — so a reader has
to know each width from the id alone. That makes the table above load-bearing rather than merely
informative, and it is why `m300-device-sim` omits an unknown status id from its reply and sets
the result field to failure, rather than guessing a width and desynchronising the reader.

The three analogue readings the simulator reports are invented but stable: there is no board to
measure, and a value that drifted would make a test flaky for no gain in fidelity.

## 7. CRC32 — and the specification's disagreement with itself

The checksum covers magic through payload inclusive, and the specification explicitly permits
sending `0x00000000` instead: "如计算资源有限可以不校验 返回0x00000000即可" — a device short of
compute may skip it.

**The specification defines the checksum twice, and the two definitions do not agree.**

| Where | What it is | Value for the example frame |
| --- | --- | --- |
| The C listing (a 256-entry table plus the loop) | standard CRC-32, reflected `0xEDB88320` | `0x5CEBDD6D` |
| The Python example (`import crc32c`, and its printed output) | CRC-32C (Castagnoli), reflected `0x82F63B78` | `0xA57105A2` |

The printed output `53435A4E0001000100000000A57105A2` settles which one the example ran; the
table settles what the document meant to specify. Only hardware settles which one the firmware
implements.

`crates/quickvib-sczn` therefore computes both. On **transmit** it writes standard CRC-32 by
default, because the table is the normative part of the document; `--crc castagnoli` and
`--crc zero` cover the other two readings. On **receive** it accepts any of the three, because
rejecting a frame on this basis would mean picking a winner in the vendor's own disagreement,
and picking wrong turns a working link into a silent one with no way to tell which end is at
fault.

**bench.** Send each mode in turn and watch for `-4 M300_ERR_CRC_MISMATCH` from the SDK
(`docs/M300-NATIVE.md` §5). Whichever mode does not produce it is the answer; if none does, the
SDK does not verify at all and `--crc zero` is sufficient forever.

## 8. Triggered capture

The specification's own sequence, for reference — `m300-device-sim` stores the trigger
parameters but always free-runs, since there is no hardware trigger to wait for:

```text
host → device   0x06 write params (0x0E trigger type, 0x0F edge, 0x10 level, 0x11 channel, 0x12 length)
device → host   0x07 reply
host → device   0x00 start acquisition
device → host   0x01 reply
                — wait for the hardware trigger —
device → host   0x04 data upload
```

A stop (`0x02`) during the wait is honoured.

## 9. What `m300-device-sim` does not do

* **No pulse output.** `0xD0`/`0xD2` are answered with a failure result.
* **No firmware upgrade.** The `0xF8`–`0xFF` family is refused. The specification's AES-256-CTR
  key and IV for the firmware CRC field are in the vendor document; nothing here uses them, and
  no crypto dependency is pulled in for them.
* **No I/Q synthesis.** See §4.
* **No UDP discovery or configuration.** See §10.
* **No `device_type = 0x02`.** See §5.1.

## 10. The UDP configuration protocol (not implemented)

A separate protocol, documented in `网络配置协议` / `net_conf_protocol.md`, for discovering and
configuring devices before they can dial in. Summarised here only so the differences are on
record and nobody reaches for the wrong codec:

| | TCP SCZN (this document) | UDP net-config |
| --- | --- | --- |
| Transport | TCP, device dials the host | UDP broadcast, host `9222` → device, device `9223` → host |
| Header | 12 bytes, no MAC | 18 bytes, includes a 6-byte target MAC |
| Byte order | **big-endian** | **little-endian**, except IP fields which are network order |
| Purpose | acquisition and parameters | addressing, server/MQTT configuration, time sync |

`docs/M300-NATIVE.md` §3 records that QuickVib deliberately binds none of the SDK's `m300_nc_*`
family, and §5 records the trap that `M300NcResult` swaps `-6` and `-7` relative to `M300Result`.
Neither applies to anything in this document.

## 11. Where the code is

| File | What it holds |
| --- | --- |
| `crates/quickvib-sczn/src/crc.rs` | Both checksum variants and the acceptance rule of §7 |
| `crates/quickvib-sczn/src/packet.rs` | The frame of §2 and a resynchronising reader for a TCP stream |
| `crates/quickvib-sczn/src/params.rs` | The ids, widths and ladders of §5 and §6 |
| `crates/quickvib-sczn/src/device.rs` | The command table of §3, as a pure state machine |
| `crates/quickvib-sczn/src/upload.rs` | The payload of §4 and the tone that fills it |
| `crates/quickvib-sczn/src/runner.rs` | The transport of §1: dial, serve, redial |
| `crates/quickvib-sczn/tests/sdk_side.rs` | A minimal host driving the whole thing over loopback |

Everything except `runner.rs` is pure, so the command set is unit-tested on any host. What no
test here can prove is that `m300_sdk.dll` agrees with the readings marked **bench** above.
