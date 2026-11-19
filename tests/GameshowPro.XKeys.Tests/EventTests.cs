// Copyright (c) Barjonas LLC 2026.
using System.Collections.Concurrent;
using System.Diagnostics;
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>
/// Push delivery of a real device (<see cref="XkDevice.Start"/>): the event thread, ordering, the one-mode rule, handlers that throw and
/// the end of a session from every side. Inconclusive when no device or DLL is present. Only Generate Data (177) is sent, and the
/// volatile indicator LED command in one test, which leaves the LEDs off. Nobody presses keys, so every report is a generated one.
/// </summary>
[TestClass]
public sealed class EventTests
{
    private const int Timeout = 2000;

    [TestMethod]
    public void AStartedDevice_RaisesTheAnswerToGenerateData_OnItsOwnThread_AndNoInputChange()
    {
        IReadOnlyList<XkDeviceDescriptor> devices = TestSupport.RequireDevices();
        TestSupport.RetryOnce(() =>
        {
            foreach (XkDeviceDescriptor descriptor in devices)
            {
                using XkDevice device = XkDevice.Open(descriptor);
                ConcurrentQueue<(XkReportEventArgs Args, Thread Thread)> reports = new();
                int inputChanges = 0;
                device.ReportReceived += (_, e) => reports.Enqueue((e, Thread.CurrentThread));
                device.InputsChanged += (_, _) => Interlocked.Increment(ref inputChanges);
                Assert.AreEqual(XkDeliveryMode.None, device.DeliveryMode);
                Assert.AreSame(XkModel.ForPid(descriptor.Pid), device.Model);

                long before = Stopwatch.GetTimestamp();
                Assert.AreEqual(XkError.Ok, device.Start());
                Assert.AreEqual(XkDeliveryMode.Events, device.DeliveryMode);
                Assert.IsTrue(SpinWait.SpinUntil(() => !reports.IsEmpty, Timeout), $"0x{descriptor.Pid:X4}: no report");
                long after = Stopwatch.GetTimestamp();
                Thread.Sleep(50);

                (XkReportEventArgs args, Thread thread) = reports.First();
                Assert.AreNotEqual(Environment.CurrentManagedThreadId, thread.ManagedThreadId);
                Assert.AreEqual($"XKeys events 0x{descriptor.Pid:X4}", thread.Name);
                Assert.IsTrue(thread.IsBackground);
                Assert.AreEqual(XkReportKind.Generated, args.Kind);
                Assert.AreEqual(descriptor.InputLength, args.Data.Length);
                Assert.AreEqual(0u, args.Stamp.Sequence);
                Assert.IsTrue(args.Stamp.Timestamp >= before && args.Stamp.Timestamp <= after, "the stamp falls inside the request/answer window");
                Assert.AreEqual(args.Entry.Qpc, args.Stamp.Timestamp);
                if (device.Model.TimestampOffset >= 0)
                {
                    Assert.IsNotNull(args.Stamp.DeviceMilliseconds);
                }
                Assert.AreEqual(0, inputChanges, "the initial state is a baseline, not changes");
            }
        });
    }

    [TestMethod]
    public void TheEventsOfOneDevice_AreSerializedOnOneThread_InReportOrder()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        TestSupport.RetryOnce(() =>
        {
            using XkDevice device = XkDevice.Open(descriptor);
            int active = 0;
            int maxActive = 0;
            ConcurrentQueue<(uint Sequence, int Thread)> seen = new();
            device.ReportReceived += (_, e) =>
            {
                int now = Interlocked.Increment(ref active);
                InterlockedMax(ref maxActive, now);
                seen.Enqueue((e.Stamp.Sequence, Environment.CurrentManagedThreadId));
                Thread.Sleep(40); // slower than the device answers, so reports queue up behind the handler
                Interlocked.Decrement(ref active);
            };
            _ = device.Start(requestState: false);
            for (int i = 0; i < 5; i++)
            {
                Assert.AreEqual(XkError.Ok, device.RequestState());
            }
            Assert.IsTrue(SpinWait.SpinUntil(() => seen.Count >= 5, 3000), $"{seen.Count} of 5 reports");
            Assert.AreEqual(1, maxActive, "never two handlers at once");
            uint[] sequences = [.. seen.Select(s => s.Sequence)];
            CollectionAssert.AreEqual(sequences.Order().ToArray(), sequences, "in report order");
            Assert.AreEqual(1, seen.Select(s => s.Thread).Distinct().Count(), "one thread");
        });
    }

    [TestMethod]
    public void PullAndEvents_AreExclusive_ForTheLifeOfADevice()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        using (XkDevice started = XkDevice.Open(descriptor))
        {
            _ = started.Start(requestState: false);
            _ = Assert.Throws<InvalidOperationException>(() => started.Read(new XkReportEntry[4]));
            _ = Assert.Throws<InvalidOperationException>(() => started.WaitHandle);
            _ = Assert.Throws<InvalidOperationException>(() => started.WaitAsync());
            _ = Assert.Throws<InvalidOperationException>(() => started.Start());
            Assert.AreEqual(XkDeliveryMode.Events, started.DeliveryMode);
        }
        using (XkDevice pulled = XkDevice.Open(descriptor))
        {
            Assert.AreEqual(0, pulled.Read(new XkReportEntry[4]));
            Assert.AreEqual(XkDeliveryMode.Pull, pulled.DeliveryMode);
            _ = Assert.Throws<InvalidOperationException>(() => pulled.Start());
            _ = pulled.WaitHandle; // still fine in pull mode
        }
        using (XkDevice waited = XkDevice.Open(descriptor))
        {
            _ = waited.WaitHandle;
            _ = Assert.Throws<InvalidOperationException>(() => waited.Start());
        }
    }

    [TestMethod]
    public void AHandlerThatThrows_IsReported_TheOtherHandlersRun_AndTheReaderGoesOn()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        TestSupport.RetryOnce(() =>
        {
            using XkDevice device = XkDevice.Open(descriptor);
            InvalidOperationException thrown = new("handler failure for the test");
            int second = 0;
            ConcurrentQueue<XkErrorEventArgs> failures = new();
            device.ReportReceived += (_, _) => throw thrown;
            device.ReportReceived += (_, _) => Interlocked.Increment(ref second);
            device.HandlerFailed += (_, e) => failures.Enqueue(e);
            device.HandlerFailed += (_, _) => throw new InvalidOperationException("a failing failure handler is dropped");

            _ = device.Start();
            Assert.IsTrue(SpinWait.SpinUntil(() => second >= 1, Timeout), "the second handler ran");
            Assert.AreEqual(XkError.Ok, device.RequestState());
            Assert.IsTrue(SpinWait.SpinUntil(() => second >= 2, Timeout), "the reader went on after a handler threw");
            Assert.IsGreaterThanOrEqualTo(2, failures.Count);
            Assert.IsTrue(failures.All(f => f.Context == nameof(XkDevice.ReportReceived) && ReferenceEquals(f.Exception, thrown)));
            Assert.IsFalse(device.IsClosed);
        });
    }

    [TestMethod]
    public void DisposingFromInsideAHandler_ReturnsAtOnce_ThenClosedFollowsOnTheSameThread_AndNothingElse()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        TestSupport.RetryOnce(() =>
        {
            XkDevice device = XkDevice.Open(descriptor);
            int reports = 0;
            long disposeTicks = -1;
            int handlerThread = 0;
            ConcurrentQueue<(XkClosedEventArgs Args, int Thread)> closed = new();
            device.ReportReceived += (_, _) =>
            {
                Interlocked.Increment(ref reports);
                handlerThread = Environment.CurrentManagedThreadId;
                Stopwatch watch = Stopwatch.StartNew();
                device.Dispose();
                disposeTicks = watch.ElapsedTicks;
            };
            device.Closed += (_, e) => closed.Enqueue((e, Environment.CurrentManagedThreadId));

            _ = device.Start();
            Assert.IsTrue(SpinWait.SpinUntil(() => !closed.IsEmpty, Timeout), "Closed was raised");
            Assert.IsLessThan(Stopwatch.Frequency, disposeTicks, "Dispose inside a handler returned at once");
            (XkClosedEventArgs args, int thread) = closed.Single();
            Assert.AreEqual(XkCloseReason.Disposed, args.Reason);
            Assert.AreEqual(XkError.Ok, args.Error);
            Assert.AreEqual(handlerThread, thread);
            Assert.AreEqual(XkCloseReason.Disposed, device.CloseReason);
            Assert.AreEqual(XkError.DeviceRemoved, device.RequestState(), "the session is released");
            Thread.Sleep(100);
            Assert.AreEqual(1, reports);
            device.Dispose();
            Assert.HasCount(1, closed);
        });
    }

    [TestMethod]
    public void DisposingFromAnotherThread_RaisesClosedOnce_BeforeItReturns_AndReleasesTheSession()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        XkDevice device = XkDevice.Open(descriptor);
        int closed = 0;
        int reportsAfterDispose = 0;
        bool disposed = false;
        XkClosedEventArgs? args = null;
        device.Closed += (_, e) =>
        {
            args = e;
            Interlocked.Increment(ref closed);
            device.Dispose(); // re-entrant: harmless
        };
        device.ReportReceived += (_, _) =>
        {
            if (Volatile.Read(ref disposed))
            {
                Interlocked.Increment(ref reportsAfterDispose);
            }
        };
        _ = device.Start();
        Thread.Sleep(100);

        device.Dispose();
        Volatile.Write(ref disposed, true);

        Assert.AreEqual(1, closed, "Closed was raised before Dispose returned");
        Assert.AreEqual(XkCloseReason.Disposed, args!.Reason);
        Assert.HasCount(0, args.PressedInputs);
        Assert.IsTrue(device.IsClosed);
        Assert.AreEqual(XkError.DeviceRemoved, device.RequestState());
        _ = Assert.Throws<ObjectDisposedException>(() => device.Write(new byte[device.OutputLength], 100, out _));
        Assert.AreEqual(XkError.DeviceRemoved, device.Lights.SetLed(XkLedIndex.Green, XkLedState.Off));
        _ = Assert.Throws<ObjectDisposedException>(() => device.Start());
        device.Dispose();
        Thread.Sleep(100);
        Assert.AreEqual(1, closed);
        Assert.AreEqual(0, reportsAfterDispose);
    }

    [TestMethod]
    public void ADeviceThatWasNeverStarted_RaisesNoEvents_AndClosesAsBefore()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        XkDevice device = XkDevice.Open(descriptor);
        int events = 0;
        device.Closed += (_, _) => Interlocked.Increment(ref events);
        device.ReportReceived += (_, _) => Interlocked.Increment(ref events);
        Assert.AreEqual(XkError.Ok, device.RequestState());
        Thread.Sleep(100);
        device.Dispose();
        Assert.AreEqual(0, events);
        Assert.AreEqual(XkCloseReason.Disposed, device.CloseReason);
    }

    [TestMethod]
    public void ManyStartAndDisposeCycles_ArePromptAndLeaveNoThreads()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        int threadsBefore = Process.GetCurrentProcess().Threads.Count;
        Stopwatch watch = Stopwatch.StartNew();
        for (int i = 0; i < 50; i++)
        {
            using XkDevice device = XkDevice.Open(descriptor);
            _ = device.Start(requestState: i % 2 == 0);
        }
        Assert.IsLessThan(10_000, watch.ElapsedMilliseconds, "50 start/dispose cycles took too long");
        Thread.Sleep(200);
        Assert.IsLessThan(threadsBefore + 10, Process.GetCurrentProcess().Threads.Count, "threads were left behind");
    }

    /// <summary>
    /// A handler that blocks while the device answers 20 requests overflows a 16-entry ring; once it returns, the device asks for the
    /// state by itself (Generate Data) and that answer arrives without the test requesting it.
    /// </summary>
    [TestMethod]
    public void LostReports_AreFollowedByAStateRequestFromTheLibrary()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        TestSupport.RetryOnce(() =>
        {
            using XkDevice device = XkDevice.Open(descriptor, ringCapacity: 16);
            using ManualResetEventSlim gate = new(false);
            ConcurrentQueue<XkReportStamp> stamps = new();
            device.ReportReceived += (_, e) =>
            {
                stamps.Enqueue(e.Stamp);
                _ = gate.Wait(5000);
            };
            _ = device.Start(requestState: false);
            for (int i = 0; i < 20; i++)
            {
                Assert.AreEqual(XkError.Ok, device.RequestState());
            }
            Thread.Sleep(200);
            Assert.IsGreaterThan(0ul, device.DroppedCount, "the ring overflowed");
            gate.Set();
            Assert.IsTrue(SpinWait.SpinUntil(() => stamps.Any(s => s.Sequence >= 20), Timeout), $"no report after the 20 requested; sequences {string.Join(",", stamps.Select(s => s.Sequence))}");
        });
    }

    /// <summary>Writes the volatile indicator command while the event thread runs, and leaves both LEDs off.</summary>
    [TestMethod]
    public void LightsCanBeWrittenWhileEventsRun()
    {
        XkDeviceDescriptor descriptor = TestSupport.RequireDevices()[0];
        using XkDevice device = XkDevice.Open(descriptor);
        int reports = 0;
        device.ReportReceived += (_, _) => Interlocked.Increment(ref reports);
        _ = device.Start();
        try
        {
            Assert.AreEqual(XkError.Ok, device.Lights.SetIndicators(XkLedState.On, XkLedState.Off));
            Assert.AreEqual(XkError.Ok, device.RequestState());
            Assert.AreEqual(XkError.Ok, device.Lights.SetIndicators(XkLedState.Off, XkLedState.Off));
        }
        finally
        {
            _ = device.Lights.SetIndicators(XkLedState.Off, XkLedState.Off);
        }
        Assert.IsTrue(SpinWait.SpinUntil(() => reports >= 2, Timeout));
    }

    private static void InterlockedMax(ref int target, int value)
    {
        int current;
        while ((current = Volatile.Read(ref target)) < value && Interlocked.CompareExchange(ref target, value, current) != current)
        {
        }
    }
}
