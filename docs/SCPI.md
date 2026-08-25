# QuickVib SCPI reference

The remote interface a UTS talks to: one `\n`-terminated line per message, `;` separates messages
inside one line, short forms are shown in uppercase and the optional completion in lowercase
(`REC:STAT?` and `RECord:STATus?` are the same command, partial completions such as `RECor:STAT?`
are not). Headers are case-insensitive. Only queries produce a response line.

This page is the operator-facing summary; `PLAN.md` §8–§9 remains the normative specification.

## Command table

| Command | Type | Response | Notes |
| --- | --- | --- | --- |
| `*IDN?` | Query | `QuickVib,M300-SCPI,<serial>,<fw>` | All four fields come from the project's `identity` block; serial falls back to the connected device's. |
| `*RST` | Command | — | Abort any run, discard the capture, restore the project's duration and format, clear the error queue, return to `IDLE`. |
| `*CLS` | Command | — | Clear the error queue and any armed `*OPC` request. Leaves the capture, the state and the loaded project alone. |
| `*OPC` | Command | — | Arm the OPC bit for when the active run finishes. There is no `*ESR?`/`*STB?`, so wait with `*OPC?`, `REC:WAIT?` or `#REC:DONE`. |
| `*OPC?` | Query | `1` | Blocks until the run finishes; bounded by the run watchdog. |
| `SYST:ERR?` | Query | `<code>,"<message>"` | Oldest queued error, `0,"No error"` when empty. `SYST:ERR:NEXT?` is an alias. |
| `SYST:VERS?` | Query | `1999.0` | SCPI standard version. |
| `SYST:DEV:CONN?` | Query | `1` \| `0` | Whether a device link is up. |
| `MMEM:LOAD:STAT "<path>"` | Command | — | Load a project file. `-256` missing, `-224` bad schema, `-221` during a run. |
| `MMEM:STOR:STAT "<path>"` | Command | — | Save the current project, runtime overrides included. `-257` if unwritable. |
| `MMEM:LOAD:AUTO` | Command | — | Load the most recently used project. `-256` when there is no usable record. |
| `MMEM:LOAD:AUTO?` | Query | `"<path>"` \| `""` | The path auto-load would use. |
| `MMEM:STOR:TRAC "<path>"` | Command | — | Export the last completed capture in the active `FORM`. `-230` without one, `-257` if unwritable. |
| `CONF:REC:DUR <seconds>` | Command | — | Record duration, range `(0, 3600]` and further bounded by the capture-size cap. `-222` out of range, `-221` during a run. |
| `CONF:REC:DUR?` | Query | `5.000` | Three decimals. |
| `FORM CSV\|TXT` | Command | — | Export format. `-224` for anything else. `FORM:DATA` is an alias. |
| `FORM?` | Query | `CSV` | Active format. |
| `INIT` | Command | — | Start a run; non-blocking, discards the previous capture. `-221` if one is already active or no project is loaded, `-241` if no device. `REC:STAR` and `INIT:IMM` are aliases. |
| `ABOR` | Command | — | Abort the active run. Silent when nothing is running. |
| `REC:STAT?` | Query | `IDLE\|ARMED\|RECORDING\|COMPLETE\|ABORTED` | Non-blocking, safe to poll. |
| `REC:WAIT?` | Query | `1` \| `0` | Blocks until the run completes (`1`) or aborts/times out (`0`); watchdog-bounded. |
| `FETC?` | Query | `v1,v2,…,vN` | The whole capture as ASCII floats. `-230` without a completed capture. `TRAC:DATA?` is an alias. |
| `TRAC:POIN?` | Query | `500000` | Sample count of that capture, for sizing the read buffer. |
| `CALC:MEAS:PEAK?` | Query | `12.3400` | `max(abs(x))`. |
| `CALC:MEAS:RMS?` | Query | `4.5600` | Root mean square. |
| `CALC:MEAS:PP?` | Query | `24.6800` | `max − min`. |
| `CALC:MEAS:ALL?` | Query | `12.3400,4.5600,24.6800` | Peak, RMS, peak-to-peak, in that order. |

Measurement queries use the project's `measurement.responseDecimals` (4 by default) and answer
`-230` when there is no completed capture.

Two unsolicited lines are pushed to every open session, so a UTS can wait on notifications instead
of polling: `#REC:DONE` when a run completes and `#REC:ABORT` when one aborts or times out.

## Errors

| Code | Message | Typical cause |
| --- | --- | --- |
| `0` | `No error` | Queue empty |
| `-100` | `Command error` | Malformed message, over-long or unterminated line, non-UTF-8 input |
| `-113` | `Undefined header` | Unknown command |
| `-221` | `Settings conflict` | No project loaded, already recording, or a configuration change during a run |
| `-222` | `Data out of range` | Duration outside `(0, 3600]` or over the capture-size cap |
| `-224` | `Illegal parameter value` | Bad `FORM` value or an invalid project file |
| `-230` | `Data corrupt or stale` | Data queried with no completed capture |
| `-240` | `Hardware error` | Device link dropped mid-run |
| `-241` | `Hardware missing` | No device connected at `INIT` |
| `-256` | `File name not found` | Project file missing |
| `-257` | `File name error` | Path invalid or not writable |
| `-350` | `Queue overflow` | More than 32 unread errors; replaces the newest entry |
| `-365` | `Time out error` | Run watchdog fired |

The queue is a 32-deep FIFO. When it overflows, the newest entry is replaced by `-350`, so the last
error a UTS reads after a burst always says that errors were lost.

## Loading a project discards the capture

`MMEM:LOAD:STAT`, `MMEM:LOAD:AUTO` and the GUI's *Apply* all replace the instrument's project, and a
capture belongs to the project it was taken under. Adopting a project therefore drops the stored
capture and returns the instrument to `IDLE`:

- `REC:STAT?` answers `IDLE`, not `COMPLETE`, even if the run before the load had finished.
- `FETC?`, `TRAC:DATA?`, `TRAC:POIN?`, `CALC:MEAS:*` and `MMEM:STOR:TRAC` answer `-230`.

Fetch or export before loading the next project. A load attempted while a run is in flight is
refused with `-221` and changes nothing.
