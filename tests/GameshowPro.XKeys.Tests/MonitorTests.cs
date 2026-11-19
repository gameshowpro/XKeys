// Copyright (c) Barjonas LLC 2026.
using System.Collections.Concurrent;
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>
/// <see cref="XkDeviceMonitor"/> against the real devices: initial arrivals, opening and handing over, start and stop, handlers that
/// throw or dispose. Unplugging cannot be automated, so removal and return are simulated by replacing the monitor's enumeration and
/// watch generation (the sessions themselves are real). Only Generate Data (177) is sent. Inconclusive without a device or the DLL.
/// </summary>
[TestClass]
public sealed class MonitorTests
{
    private const int Timeout = 3000;

    private static List<XkDeviceDescriptor> Present() => [.. XkEnumerator.Enumerate().Where(d => d.InputLength > 0)];

    [TestMethod]
    public void Start_FillsThePresentSet_ThenRaisesAnArrivalForEach_OnTheMonitorThread_InOrder()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        using XkDeviceMonitor monitor = new();
        ConcurrentQueue<(XkDeviceEventArgs Args, Thread Thread)> arrivals = new();
        monitor.DeviceArrived += (_, e) => arrivals.Enqueue((e, Thread.CurrentThread));

        monitor.Start();

        CollectionAssert.AreEqual(present.ToList(), monitor.PresentDevices.ToList(), "filled when Start returns, in the library's order");
        Assert.IsTrue(SpinWait.SpinUntil(() => arrivals.Count == present.Count, Timeout));
        CollectionAssert.AreEqual(present.ToList(), arrivals.Select(a => a.Args.Descriptor).ToList());
        Assert.IsTrue(arrivals.All(a => a.Args.WasPresentAtStart));
        Assert.IsTrue(arrivals.All(a => a.Thread.Name == "XKeys device monitor" && a.Thread.IsBackground));
        Assert.AreEqual(XkModel.ForPid(present[0].Pid), arrivals.First().Args.Model);
        Assert.HasCount(0, monitor.OpenDevices, "OpenDevices is off by default");
        monitor.Rescan();
        Thread.Sleep(200);
        Assert.HasCount(present.Count, arrivals, "nothing changed, so nothing is raised again");
    }

    [TestMethod]
    public void StartAndDispose_FollowTheirRules()
    {
        TestSupport.RequireDll();
        XkDeviceMonitor neverStarted = new();
        neverStarted.Dispose();
        neverStarted.Dispose();
        _ = Assert.Throws<ObjectDisposedException>(neverStarted.Start);

        XkDeviceMonitor monitor = new();
        monitor.Start();
        _ = Assert.Throws<InvalidOperationException>(monitor.Start);
        monitor.Dispose();
        monitor.Dispose();
        monitor.Rescan(); // harmless after dispose
    }

    [TestMethod]
    public void OpenDevices_HandsOverEachDeviceUnstarted_StartsItAfterTheHandler_AndItDeliversItsState()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        TestSupport.RetryOnce(() =>
        {
            ConcurrentQueue<XkDevice> opened = new();
            ConcurrentDictionary<string, XkReportEventArgs> firstReport = new(StringComparer.OrdinalIgnoreCase);
            ConcurrentQueue<XkCloseReason> closed = new();
            XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true });
            ConcurrentQueue<(XkDeliveryMode Mode, bool IsReopen)> inHandler = new();
            monitor.DeviceOpened += (_, e) =>
            {
                // Recorded and checked outside: an assertion inside a handler would only reach HandlerFailed.
                inHandler.Enqueue((e.Device.DeliveryMode, e.IsReopen));
                e.Device.ReportReceived += (_, r) => firstReport.TryAdd(e.Device.Path, r);
                e.Device.Closed += (_, c) => closed.Enqueue(c.Reason);
                opened.Enqueue(e.Device);
            };
            try
            {
                monitor.Start();
                Assert.IsTrue(SpinWait.SpinUntil(() => firstReport.Count == present.Count, Timeout), $"{firstReport.Count} of {present.Count} devices delivered their state");
                Assert.IsTrue(firstReport.Values.All(r => r.Kind == XkReportKind.Generated));
                Assert.IsTrue(inHandler.All(h => h.Mode == XkDeliveryMode.None && !h.IsReopen), "not started before the handler returned");
                Assert.IsTrue(opened.All(d => d.DeliveryMode == XkDeliveryMode.Events));
                CollectionAssert.AreEqual(present.Select(d => d.Path).ToList(), monitor.OpenDevices.Select(d => d.Path).ToList());
            }
            finally
            {
                monitor.Dispose();
            }
            Assert.HasCount(present.Count, closed, "disposing the monitor closes every device it opened, before it returns");
            Assert.IsTrue(closed.All(r => r == XkCloseReason.Disposed));
            Assert.IsTrue(opened.All(d => d.IsClosed));
            Assert.HasCount(0, monitor.OpenDevices);
        });
    }

    [TestMethod]
    public void AHandlerThatChoosesPull_KeepsTheDeviceInPullMode()
    {
        TestSupport.RequireDevices();
        using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true });
        ConcurrentQueue<XkDevice> opened = new();
        monitor.DeviceOpened += (_, e) =>
        {
            _ = e.Device.WaitHandle;
            opened.Enqueue(e.Device);
        };
        monitor.Start();
        Assert.IsTrue(SpinWait.SpinUntil(() => !opened.IsEmpty, Timeout));
        Thread.Sleep(100);
        Assert.IsTrue(opened.All(d => d.DeliveryMode == XkDeliveryMode.Pull));
    }

    /// <summary>An unplug and replug, simulated: the interface leaves the enumeration (its session ends as removed) and comes back (reopened).</summary>
    [TestMethod]
    public void ARemovedInterface_EndsItsSessionAsRemoved_AndIsReopenedWhenItReturns()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        TestSupport.RetryOnce(() =>
        {
            XkDeviceDescriptor victim = present[0];
            List<XkDeviceDescriptor> listed = [.. present];
            ulong generation = 1;
            using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true })
            {
                EnumerateOverride = () => { lock (listed) { return [.. listed]; } },
                GenerationOverride = () => Interlocked.Read(ref generation),
            };
            ConcurrentQueue<XkDeviceOpenedEventArgs> opened = new();
            ConcurrentQueue<XkDeviceEventArgs> removed = new();
            ConcurrentQueue<XkDeviceEventArgs> arrived = new();
            ConcurrentQueue<XkCloseReason> victimClosed = new();
            ConcurrentQueue<uint> reopenedReports = new();
            monitor.DeviceOpened += (_, e) =>
            {
                opened.Enqueue(e);
                if (e.Device.Path == victim.Path)
                {
                    e.Device.Closed += (_, c) => victimClosed.Enqueue(c.Reason);
                    if (e.IsReopen)
                    {
                        e.Device.ReportReceived += (_, r) => reopenedReports.Enqueue(r.Stamp.Sequence);
                    }
                }
            };
            monitor.DeviceRemoved += (_, e) => removed.Enqueue(e);
            monitor.DeviceArrived += (_, e) => arrived.Enqueue(e);
            monitor.Start();
            Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count, Timeout));

            lock (listed)
            {
                listed.Remove(victim);
            }
            Interlocked.Increment(ref generation);
            monitor.Rescan();
            Assert.IsTrue(SpinWait.SpinUntil(() => !removed.IsEmpty && !victimClosed.IsEmpty, Timeout), "removal");
            Assert.AreEqual(victim, removed.Single().Descriptor);
            Assert.AreEqual(XkCloseReason.Removed, victimClosed.Single());
            Assert.IsFalse(monitor.PresentDevices.Contains(victim));
            Assert.IsFalse(monitor.OpenDevices.Any(d => d.Path == victim.Path));

            lock (listed)
            {
                listed.Add(victim);
            }
            Interlocked.Increment(ref generation);
            monitor.Rescan();
            Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count + 1, Timeout), "reopened");
            XkDeviceOpenedEventArgs again = opened.Last();
            Assert.AreEqual(victim.Path, again.Device.Path);
            Assert.IsTrue(again.IsReopen);
            Assert.IsFalse(arrived.Last().WasPresentAtStart);
            Assert.IsTrue(SpinWait.SpinUntil(() => !reopenedReports.IsEmpty, Timeout), "the reopened session delivered its state");
            Assert.AreEqual(0u, reopenedReports.First(), "a new session");
        });
    }

    [TestMethod]
    public void ASessionTheConsumerDisposes_IsLeftAlone_UntilItsInterfaceLeavesAndReturns()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        XkDeviceDescriptor victim = present[0];
        List<XkDeviceDescriptor> listed = [.. present];
        ulong generation = 1;
        using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true })
        {
            EnumerateOverride = () => { lock (listed) { return [.. listed]; } },
            GenerationOverride = () => Interlocked.Read(ref generation),
        };
        ConcurrentQueue<XkDeviceOpenedEventArgs> opened = new();
        monitor.DeviceOpened += (_, e) => opened.Enqueue(e);
        monitor.Start();
        Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count, Timeout));

        opened.First(o => o.Device.Path == victim.Path).Device.Dispose();
        Interlocked.Increment(ref generation);
        monitor.Rescan();
        Thread.Sleep(300);
        Assert.HasCount(present.Count, opened, "not reopened while it stays");

        lock (listed)
        {
            listed.Remove(victim);
        }
        Interlocked.Increment(ref generation);
        monitor.Rescan();
        Thread.Sleep(200);
        lock (listed)
        {
            listed.Add(victim);
        }
        Interlocked.Increment(ref generation);
        monitor.Rescan();
        Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count + 1, Timeout), "reopened after it left and came back");
        Assert.IsTrue(opened.Last().IsReopen);
    }

    /// <summary>A session that ends by itself (here: ended as a read error would end it) is not reopened in a loop; it is after the next notification.</summary>
    [TestMethod]
    public void ASessionThatEndsByItself_IsReopenedOnlyAfterTheNextNotification()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        ulong generation = 1;
        using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true })
        {
            GenerationOverride = () => Interlocked.Read(ref generation),
        };
        ConcurrentQueue<XkDeviceOpenedEventArgs> opened = new();
        monitor.DeviceOpened += (_, e) => opened.Enqueue(e);
        monitor.Start();
        Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count, Timeout));

        XkDevice first = opened.First().Device;
        first.Close(XkCloseReason.ReadError);
        Assert.AreEqual(XkCloseReason.ReadError, first.CloseReason);
        Thread.Sleep(300);
        Assert.HasCount(present.Count, opened, "no arrival or removal since it opened: not reopened");

        Interlocked.Increment(ref generation);
        monitor.Rescan();
        Assert.IsTrue(SpinWait.SpinUntil(() => opened.Count == present.Count + 1, Timeout), "reopened after a notification");
        Assert.AreEqual(first.Path, opened.Last().Device.Path);
        Assert.IsTrue(opened.Last().IsReopen);
    }

    [TestMethod]
    public void TheMonitorKeepsTheLibrarysOrder_WhateverOrderTheEnumerationGives()
    {
        TestSupport.RequireDll();
        List<XkDeviceDescriptor> scrambled = [OrderTests.SecondSwitchInterface, OrderTests.JogAndShuttle, OrderTests.SwitchInterface, OrderTests.SecondJogAndShuttle];
        using XkDeviceMonitor monitor = new() { EnumerateOverride = () => scrambled };
        ConcurrentQueue<XkDeviceDescriptor> arrivals = new();
        monitor.DeviceArrived += (_, e) => arrivals.Enqueue(e.Descriptor);
        monitor.Start();
        XkDeviceDescriptor[] expected = [OrderTests.SecondJogAndShuttle, OrderTests.JogAndShuttle, OrderTests.SwitchInterface, OrderTests.SecondSwitchInterface];
        CollectionAssert.AreEqual(expected, monitor.PresentDevices.ToList());
        Assert.IsTrue(SpinWait.SpinUntil(() => arrivals.Count == 4, Timeout));
        CollectionAssert.AreEqual(expected, arrivals.ToList());
    }

    [TestMethod]
    public void AMonitorHandlerThatThrows_IsReported_AndTheMonitorGoesOn()
    {
        IReadOnlyList<XkDeviceDescriptor> present = TestSupport.RequireDevices();
        using XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true, RequestStateOnStart = false });
        InvalidOperationException thrown = new("monitor handler failure for the test");
        ConcurrentQueue<XkErrorEventArgs> failures = new();
        int opened = 0;
        monitor.DeviceArrived += (_, _) => throw thrown;
        monitor.HandlerFailed += (_, e) => failures.Enqueue(e);
        monitor.DeviceOpened += (_, _) => Interlocked.Increment(ref opened);
        monitor.Start();
        Assert.IsTrue(SpinWait.SpinUntil(() => opened == present.Count, Timeout), "devices were still opened");
        Assert.HasCount(present.Count, failures);
        Assert.IsTrue(failures.All(f => f.Context == nameof(XkDeviceMonitor.DeviceArrived) && ReferenceEquals(f.Exception, thrown)));
    }

    [TestMethod]
    public void DisposingTheMonitorFromItsOwnHandler_ReturnsAtOnce_AndStopsIt()
    {
        TestSupport.RequireDevices();
        XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true });
        int arrivals = 0;
        int opened = 0;
        using ManualResetEventSlim returned = new(false);
        monitor.DeviceArrived += (_, _) =>
        {
            Interlocked.Increment(ref arrivals);
            monitor.Dispose();
            returned.Set();
        };
        monitor.DeviceOpened += (_, _) => Interlocked.Increment(ref opened);
        monitor.Start();
        Assert.IsTrue(returned.Wait(Timeout), "Dispose inside a handler returned");
        Thread.Sleep(200);
        Assert.AreEqual(1, arrivals, "nothing after the dispose");
        Assert.AreEqual(0, opened);
        monitor.Dispose();
    }

    [TestMethod]
    public void DisposingTheMonitorFromADeviceHandler_DoesNotDeadlock_AndClosesTheDevice()
    {
        TestSupport.RequireDevices();
        XkDeviceMonitor monitor = new(new XkDeviceMonitorOptions { OpenDevices = true });
        ConcurrentQueue<XkCloseReason> closed = new();
        int disposedOnce = 0;
        monitor.DeviceOpened += (_, e) =>
        {
            e.Device.Closed += (_, c) => closed.Enqueue(c.Reason);
            e.Device.ReportReceived += (_, _) =>
            {
                if (Interlocked.Exchange(ref disposedOnce, 1) == 0)
                {
                    monitor.Dispose();
                }
            };
        };
        monitor.Start();
        Assert.IsTrue(SpinWait.SpinUntil(() => disposedOnce == 1, Timeout), "a device delivered a report");
        Assert.IsTrue(SpinWait.SpinUntil(() => monitor.OpenDevices.Count == 0 && !closed.IsEmpty, Timeout), "the monitor closed its devices");
        Assert.IsTrue(closed.All(r => r == XkCloseReason.Disposed));
        monitor.Dispose();
    }
}
