# Device measurements

Facts measured on real X-keys devices, which the code and its tests rely on. Everything here was measured on a test machine with
one XK-12 Jog & Shuttle (PID 0x0426) and one XK-12 Switch Interface (PID 0x04AB) attached, on 2026-11-15 and 2026-11-16, with the
diagnostic tool in `src/GameshowPro.XKeys.Native/examples/xkprobe.rs`: `xkprobe list` and `xkprobe probe` (read-only capability
queries, and reports after a Generate Data request; command 177 was the only command sent), and `xkprobe guide` (a guided manual
test with a person at the devices). Where a fact comes only from P.I. Engineering's published SDK documentation, the code says so.

## Device capabilities

Recorded 2026-11-15 with a read-only capabilities query (`xkprobe list`).

| Device | PID | Interface | Usage page / usage | Input / output report bytes |
|---|---|---|---|---|
| XK-12 Jog & Shuttle (version 0x0100) | 0x0426 | MI_00 consumer control (the one the library uses) | 0xC / 0x1 | 33 / 36 |
| | | MI_01 keyboard | 0x1 / 0x6 | 9 / 2 |
| | | MI_02 mouse | 0x1 / 0x2 | 6 / 0 |
| XK-12 Switch Interface (version 0x0000) | 0x04AB | MI_00 consumer control (the one the library uses) | 0xC / 0x1 | 37 / 36 |
| | | MI_01 keyboard | 0x1 / 0x6 | 9 / 2 |
| | | MI_02 mouse | 0x1 / 0x2 | 7 / 0 |

- Windows' HID input queue is 32 reports on every interface (the default).
- Manufacturer string "P. I. Engineering"; product strings "XK-12 Jog & Shuttle" and "PI4 Switch Interface". Feature report
  length is 0 on every interface.
- The Switch Interface is the XK-12 variant (PID 0x04AB is the fourth of 0x04A8 to 0x04AB).
- Consumer-control device paths have the form `\\?\hid#vid_05f3&pid_0426&mi_00#...#{4d1e55b2-...}` (vendor ID 0x05F3).

## Measured with `xkprobe probe`

Raw input reports after a Generate Data request (index 0 first; five requests, 500 ms apart):

```
J&S #0  00 02 02 00 00 80 00 00 00 00 0D 1C BE 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
J&S #1  00 02 02 00 00 80 00 00 00 00 0D 1E AC 00 ...   (index 9..12: 859326 -> 859820 = +494 ms)
SI  #0  00 01 02 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 0C E9 BE 01
SI  #1  00 01 02 00 ... 00 0C EB AE 01                   (index 32..35: 846270 -> 846766 = +496 ms)
```

- Index 1 is the unit ID (Jog & Shuttle 2, Switch Interface 1). Index 2 is 2 (a generated report; the Jog & Shuttle's program
  switch is unset). Jog & Shuttle index 5 is 0x80 (shuttle at rest).
- **Jog & Shuttle timestamp: indexes 9 to 12, big-endian.** Reading indexes 10 to 13 gives values 256 times too large, with
  index 13 always 0.
- **Switch Interface timestamp: indexes 32 to 35, big-endian;** index 36 is 0x01. The timestamp was enabled as found on this unit
  (no command 210 was sent, so whether that is the factory default or an earlier setting is not known). The Jog & Shuttle's
  timestamp was on too.
- Both counters read about 14 minutes at the first probe, consistent with milliseconds since plug-in.
- Device clock against the host's performance counter: ratio 0.9935 to 0.9983 over 5 to 20 s runs, so both units ran 0.2 to 0.7 %
  slow (Jog & Shuttle 19869 device ms over 19999.6 ms; Switch Interface 19897 over 20001.7). Within 1 ms over a 0.55 s burst; at
  1 s spacing single intervals read 989 to 1001 device ms. The device timestamp is good for ordering reports from one device over
  short spans, not as a clock and not across devices.
- **USB polling interval.** Endpoint descriptors from the hub's cached connection information (read-only; nothing is sent to the
  device; a one-off query): both devices are full-speed USB 1.1 (bcdUSB 0x0110). Jog & Shuttle consumer IN endpoint 0x83, max
  packet 32, **bInterval 10**; Switch Interface IN endpoint 0x83, max packet 36, **bInterval 1**; OUT endpoint 0x04, max packet 35,
  bInterval 1 on both. Windows schedules a full-speed interrupt endpoint at the largest power of two not above bInterval, so the
  Jog & Shuttle is polled every **8 ms** and the Switch Interface every **1 ms**. The host stamps show exactly that: every interval
  between Jog & Shuttle reports is within 70 us of a multiple of 8 ms (for example 503986, 503997 and 496002 us), and every Switch
  Interface interval is within 76 us of a multiple of 1 ms and not on an 8 ms grid.
- Host stamp jitter: interval residuals against that grid have a standard deviation of 11 to 15 us (about 8 to 11 us per stamp),
  worst case 76 us, on an idle machine. Raising the read thread's priority through MMCSS made no measurable difference.
- Generate Data latency (write complete to report stamp): Switch Interface 0.7 to 3.8 ms, Jog & Shuttle 7.7 to 17.8 ms. A single
  36-byte write takes 1.4 to 1.9 ms.
- Requests in a row: the first two writes complete in 1.6 and 2.0 ms, then every write blocks for 28 to 29 ms. The device answers
  one Generate Data per **29 ms** of its own clock (device-clock deltas 28 to 30 ms on both units). That is a limit on generated
  reports, not the polling interval.

## Measured with `xkprobe guide` (2026-11-16)

A person at the devices, following the tool's prompts.

- **Jog & Shuttle keys:** all 12 mapped. Four keys per row in three rows: row position is report index 3 to 6, row is bit 0 to 2.
  Input numbers 0, 8, 16, 24 (top row), 1, 9, 17, 25 (middle) and 2, 10, 18, 26 (bottom). The matrix is therefore 4 columns by
  3 rows.
- **Jog:** one report per click; index 7 is the signed step. Clockwise is +1 with index 3 bit 7 pulsed (input 7);
  counter-clockwise is -1 with index 4 bit 7 pulsed (input 15). These bits are not keys.
- **Shuttle:** index 8 is the signed position, +1 to +7 clockwise and -1 to -7 counter-clockwise. On release the spring overshoots
  zero briefly to the other side (a -1 was seen after a clockwise sweep).
- **Switch Interface (XK-12, PID 0x04AB):** 6 sockets with two inputs each. Socket n uses inputs 2(n-1) and 2(n-1)+1 (index 3
  bits 0 to 7 and index 4 bits 0 to 3), so the inputs are contiguous 0 to 11. With a mono (tip-sleeve) plug, the even input reads
  ON for as long as the plug is in, and the odd input is the button (ON when pressed). A trigger assigned to an even input
  therefore fires on plugging in.
- **Two controls at once:** seven pairs of consecutive key reports (two keys pressed or released together) in one session: 29 to
  30 ms apart by the device's clock (29 ms five times, 30 ms twice) and 24 to 32 ms by the host stamps (on the 8 ms polling grid),
  never closer. A small sample; the firmware may allow less.
- **Lights:** indicators on both devices (on, flash, faster flash, off; command 186 on the Jog & Shuttle) behaved as documented.
  Backlights: bank 2 rows light keys 1 to 4, 5 to 8 and 9 to 12 for the vendor documentation's Figure 1 indexes (the figure is
  right; the page's prose example is wrong). Bank 1 was exercised by the all-on, intensity and flashing-key questions, which
  passed; its per-key question went unanswered. Command 182 with row 1 lights the top row (keys 1 to 4) in either bank, so "rows"
  are physical rows. Toggle (184) and intensity per bank (187) work. Outputs OUT 1 and OUT 2 were not tested (nothing was
  connected). Everything was switched back off afterward.
