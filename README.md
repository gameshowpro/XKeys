# GameshowPro.XKeys

A .NET library that wraps selected [P.I. Engineering X-keys](https://xkeys.com) devices. It provides a settings system, and for WPF applications a UI template, so that X-keys devices are treated consistently with other `IncomingTriggerDevice` implementations. Underneath is our own native HID layer, which stamps every report with the performance counter and tells the library when devices arrive or leave.

## Packages

One library, built two ways from the same code (the convention of GameshowPro.Common). Both packages run on **Windows x64 only** and carry the native library `xkhid.dll`.

- **GameshowPro.XKeys** (`net11.0`): the library with no UI, for services, console programs, ASP.NET apps and any other application without WPF. Depends on `GameshowPro.Common.JsonNet`.
- **GameshowPro.XKeys.Windows** (`net11.0-windows`): the same library plus the WPF resource dictionary `GameshowPro.XKeys;component/View/Template.xaml`, for WPF applications. Depends on `GameshowPro.Common.JsonNet.Windows`.

Nothing else differs between them. Each contains the device and trigger classes (`XKeysDevices<T>`, `XKeysDevice<T>`, `XKeysIncomingTrigger`, `XKeysDeviceSettings`) and, in the namespace `GameshowPro.XKeys.Hid`, raw device access: the device monitor, open devices with decoded and stamped reports, the model catalog `XkModel`, and the lights.

Both are on nuget.org ([GameshowPro.XKeys](https://www.nuget.org/packages/GameshowPro.XKeys), [GameshowPro.XKeys.Windows](https://www.nuget.org/packages/GameshowPro.XKeys.Windows)) from version 2.2, along with the GameshowPro.Common packages they depend on, so no other package source is needed:

```
dotnet add package GameshowPro.XKeys           # no WPF
dotnet add package GameshowPro.XKeys.Windows   # WPF applications
```

`GameshowPro.XKeys` is still Windows-only at run time, and its assembly is marked `[SupportedOSPlatform("windows")]`. A project that targets plain `net11.0` either targets `net11.0-windows` instead or declares that it runs on Windows (`[assembly: SupportedOSPlatform("windows")]`, or the equivalent `AssemblyAttribute` item in the project file); otherwise it gets platform warnings (CA1416) wherever it uses the library. See [docs/migrating-from-1.md](docs/migrating-from-1.md#without-wpf).

Moving an application from 1.x: see [docs/migrating-from-1.md](docs/migrating-from-1.md). A WPF application moves to `GameshowPro.XKeys.Windows` 2.1 or later.

## No polling

Nothing in the library, and nothing it asks of you, runs on a timer. `XKeysDevices<T>.GetDevices` returns devices that are kept attached by themselves: they connect, disconnect and reconnect as devices are plugged in and out, and their triggers change and fire from the devices' own reports. Each trigger that fires because of a reported change carries that report's time in `Time` (a `TimeSpan` on the `Stopwatch` clock); inputs that changed in one report get the same time, because the device polled them together.

For raw access, let the device monitor open devices and subscribe to their events:

```csharp
using GameshowPro.XKeys.Hid;

using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true });
monitor.DeviceOpened += (_, e) =>
{
    // Subscribe here: the monitor starts the device when this handler returns. It reopens the device after an unplug and replug.
    XkDevice device = e.Device;
    device.InputsChanged += (_, group) =>
    {
        // One report: every change in it has the same stamp and cannot be ordered against the others.
        foreach (XkInputChange change in group.Changes)
        {
            Console.WriteLine($"{device.Model.InputName(change.Input)} {(change.IsPressed ? "down" : "up")} at {change.Stamp.ToTimeSpan()}");
        }
    };
    device.Closed += (_, closed) => Console.WriteLine($"{device.Model.Name} closed: {closed.Reason}");
};
monitor.Start(); // DeviceArrived and DeviceOpened for each device present now, then for each that arrives later
```

Events come from library-owned background threads: the monitor's on its own thread, each device's on that device's thread, one at a time and in report order. Keep handlers quick; an exception from a handler is caught and reported through `HandlerFailed`. The XML documentation of `XkDevice` and `XkDeviceMonitor` gives the whole threading contract. Jog and shuttle movements have their own events (`JogMoved`, `ShuttleMoved`), and `ReportReceived` gives every raw report with its stamp.

## Choosing a device by index

The library defines one order for X-keys devices: by product ID, then by device interface path (`XkDeviceDescriptor.Order`). `XkEnumerator.Enumerate`, `XkDeviceMonitor.PresentDevices` and the list from `XKeysDevices<T>.GetDevices` all use it; `GetDevices` puts the devices present at the call first, in that order, followed by a disconnected entry for each settings object no present device took. So "device 2" can be offered to a user as index 1 of any of these lists.

What is stable: the same devices on the same USB ports give the same order after a restart, a new enumeration or an unplug and replug on the same port, because Windows derives the path of a USB device without a serial number from the port it is plugged into. An entry of the `GetDevices` list keeps its index for the life of the list, and a device that is unplugged and plugged back in returns to the same entry. What is not stable: adding or removing a device shifts the devices that sort after it (the next time a list is built), moving a device to another port or hub can change its place among devices with the same product ID, and changing a device's PID mode changes its product ID and so its place.

## Building from source

Building needs the Rust toolchain (`cargo`, stable, `x86_64-pc-windows-msvc`) as well as the .NET SDK pinned in `global.json`; the build compiles `src/GameshowPro.XKeys.Native` with cargo. Choose the build with the configuration, and build or test one at a time (they share the restore output):

```
dotnet test XKeys.slnx -c Release          # GameshowPro.XKeys
dotnet test XKeys.slnx -c ReleaseWindows   # GameshowPro.XKeys.Windows
dotnet pack src/GameshowPro.XKeys -c Release -o publish
dotnet pack src/GameshowPro.XKeys -c ReleaseWindows -o publish
```

The hardware tests need an X-keys device and report themselves inconclusive without one. They send only Generate Data and volatile light commands, and leave the lights off.

The project is written in en-US. `bash scripts/check-en-us.sh` (run on every push and pull request) fails on en-GB spelling; put `en-us-allow` on a line for a deliberate exception.

## Why this library has its own native HID layer

X-keys devices are standard USB HID devices. P.I. Engineering publishes a managed wrapper, `PIEHidNetCore.dll`, and this library used it up to version 1. From version 2 it uses our own native layer (a small Rust library, `xkhid`, in `src/GameshowPro.XKeys.Native`) for the reasons below.

**Status:** from version 2.0 the library neither contains nor uses `PIEHidNetCore.dll`. Version 2.0 targets .NET 11 only. P.I. Engineering has since merged our fix for the leak described below into their source (2026-11-16, [piengineering/PIEHidNetCore.dll#2](https://github.com/piengineering/PIEHidNetCore.dll/pull/2)).

### The memory leak

A long-running Windows service that uses the vendor library was seen to grow by about 4.6 MB of private memory per hour over 20 hours, with thread and handle counts flat. The service calls `PIEDevice.EnumeratePIE()` once a second while no X-keys device is connected.

These findings come from reading the vendor's published source as it stood before that fix ([`PIEDevice.cs` at e6664f8](https://github.com/piengineering/PIEHidNetCore.dll/blob/e6664f8ecb1bb729cb7802754dd0bef66a865203/PIEHidNetCore/PIEDevice.cs)) and from observing the behavior with testing tools. In that version:

- `EnumeratePIE` calls `Marshal.AllocHGlobal` once for each HID interface on the system, for the device-path buffer. Nothing calls `Marshal.FreeHGlobal`.
- `EnumeratePIE` calls `HidD_GetPreparsedData` for each P.I. Engineering interface it opens. `HidD_FreePreparsedData` is declared but never called.
- File handles and the device information set are released, which is why handle counts stay flat while memory grows.

So every enumeration leaks native memory in proportion to the number of HID interfaces present. The observed growth works out to roughly 1.3 KB per call, which is in line with a machine that has a handful of HID interfaces. We have not yet run an isolated test that attributes the whole of the observed growth to this cause; the finding above is from the source, and the growth figure is from a service that also does other work.

The leak is inside the vendor assembly, so a caller of an affected build cannot fix it. A caller can only enumerate less often, or use a build that includes the fix.

### Timing

This library is used for game-show buzzers, where the order of two presses matters. The vendor's published source shows that it does not timestamp reports and does not set a thread priority: a report passes through a managed queue and a second managed thread before the caller sees it, so any timestamp the caller takes can be delayed by a garbage collection. The native layer stamps each input report with the performance counter on a dedicated native thread at the moment the read completes, and passes the device's own millisecond timestamp through alongside it.

A host-side timestamp cannot resolve presses closer together than the device's USB polling interval, whoever takes it.

### Device changes

The vendor library is polled to find devices. The native layer uses Windows device-change notifications, so nothing is enumerated on a timer.

### How the replacement is written

It is a new implementation from Microsoft's HID documentation and P.I. Engineering's published report formats and SDK documentation. The vendor's published source and SDK documentation were read to confirm interface facts, such as report lengths and which HID interface carries the data. No vendor code is copied. The device facts we measured ourselves are in [docs/device-measurements.md](docs/device-measurements.md).

### If you still use the vendor library

- Use a build of `PIEHidNetCore.dll` made from source that includes the fix above; builds made before it leak as described.
- Do not call `PIEDevice.EnumeratePIE()` on a short timer. Enumerate at startup and on a device-change notification, or back the interval off to many seconds.
- Watch private bytes, not managed heap size: the leak is native and does not show in .NET garbage-collector counters.

## Trademarks

X-keys is a trademark of P.I. Engineering. This project is not affiliated with or endorsed by P.I. Engineering.

## License

MIT. See [LICENSE](LICENSE). Copyright (c) 2026 Barjonas LLC.
