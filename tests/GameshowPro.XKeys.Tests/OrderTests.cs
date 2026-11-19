// Copyright (c) Barjonas LLC 2026.
using GameshowPro.XKeys.Hid;

namespace GameshowPro.XKeys.Tests;

/// <summary>
/// The library's device order (product ID, then path, ordinal and case-insensitive), from recorded paths. The first two paths were
/// recorded on a test machine (2026-11-15); the second Jog &amp; Shuttle and the second Switch Interface are made up in the same form, as
/// identical units on other ports would appear. No device needed.
/// </summary>
[TestClass]
public sealed class OrderTests
{
    private const string HidClass = "{4d1e55b2-f16f-11cf-88cb-001111000030}";

    internal static readonly XkDeviceDescriptor JogAndShuttle = Make(0x0426, $@"\\?\hid#vid_05f3&pid_0426&mi_00#c&8a9fe31&0&0000#{HidClass}", 33);
    internal static readonly XkDeviceDescriptor SwitchInterface = Make(0x04AB, $@"\\?\hid#vid_05f3&pid_04ab&mi_00#b&1edb37c8&0&0000#{HidClass}", 37);
    internal static readonly XkDeviceDescriptor SecondJogAndShuttle = Make(0x0426, $@"\\?\HID#VID_05F3&PID_0426&MI_00#7&2B1C3D4E&0&0000#{HidClass}", 33);
    internal static readonly XkDeviceDescriptor SecondSwitchInterface = Make(0x04AB, $@"\\?\hid#vid_05f3&pid_04ab&mi_00#d&11aa22bb&0&0000#{HidClass}", 37);

    internal static XkDeviceDescriptor Make(ushort pid, string path, ushort inputLength)
        => new(path, 0x05F3, pid, 0, 0x0C, 1, inputLength, 36, 0, "P. I. Engineering", "test");

    private static readonly XkDeviceDescriptor[] s_expected = [SecondJogAndShuttle, JogAndShuttle, SwitchInterface, SecondSwitchInterface];

    [TestMethod]
    public void DevicesSortByPid_ThenByPathIgnoringCase_WhateverTheInputOrder()
    {
        foreach (IEnumerable<XkDeviceDescriptor> permutation in Permutations(s_expected))
        {
            List<XkDeviceDescriptor> list = [.. permutation];
            list.Sort(XkDeviceDescriptor.Order);
            CollectionAssert.AreEqual(s_expected, list);
        }
    }

    [TestMethod]
    public void TwoUnitsWithTheSamePid_AreOrderedByPath_RegardlessOfCase()
    {
        // "7&2B1C..." < "c&8a9f..." ordinally ignoring case ('7' < 'C'); upper-casing the recorded path changes nothing.
        Assert.IsLessThan(0, XkDeviceDescriptor.Order.Compare(SecondJogAndShuttle, JogAndShuttle));
        XkDeviceDescriptor upper = JogAndShuttle with { Path = JogAndShuttle.Path.ToUpperInvariant() };
        Assert.AreEqual(0, XkDeviceDescriptor.Order.Compare(JogAndShuttle, upper));
        Assert.AreEqual(upper.SortKey, JogAndShuttle.SortKey);
    }

    [TestMethod]
    public void TheProductIdComesBeforeThePath()
    {
        // The instance part of the Switch Interface's recorded path ("b&...") sorts before the Jog & Shuttle's ("c&..."), but its PID is higher.
        Assert.IsLessThan(0, string.Compare(SwitchInterface.Path[30..], JogAndShuttle.Path[30..], StringComparison.OrdinalIgnoreCase));
        Assert.IsGreaterThan(0, XkDeviceDescriptor.Order.Compare(SwitchInterface, JogAndShuttle));
    }

    [TestMethod]
    public void TheSortKey_ShowsTheKey_AndOrdersTheSameWay()
    {
        Assert.AreEqual($@"0426 \\?\HID#VID_05F3&PID_0426&MI_00#C&8A9FE31&0&0000#{HidClass.ToUpperInvariant()}", JogAndShuttle.SortKey);
        string[] keys = [.. s_expected.Reverse().Select(d => d.SortKey).Order(StringComparer.Ordinal)];
        CollectionAssert.AreEqual(s_expected.Select(d => d.SortKey).ToArray(), keys);
    }

    [TestMethod]
    public void NullSortsFirst()
    {
        Assert.IsLessThan(0, XkDeviceDescriptor.Order.Compare(null, JogAndShuttle));
        Assert.IsGreaterThan(0, XkDeviceDescriptor.Order.Compare(JogAndShuttle, null));
        Assert.AreEqual(0, XkDeviceDescriptor.Order.Compare(null, null));
    }

    /// <summary>The real enumeration is in the library's order (both devices on the test machine: the Jog &amp; Shuttle first).</summary>
    [TestMethod]
    public void TheEnumerationIsInTheLibrarysOrder()
    {
        TestSupport.RequireDll();
        IReadOnlyList<XkDeviceDescriptor> all = XkEnumerator.Enumerate(usagePage: 0, outputLength: 0);
        CollectionAssert.AreEqual(all.Order(XkDeviceDescriptor.Order).ToList(), all.ToList());
        IReadOnlyList<XkDeviceDescriptor> consumer = XkEnumerator.Enumerate();
        CollectionAssert.AreEqual(consumer.Order(XkDeviceDescriptor.Order).ToList(), consumer.ToList());
    }

    private static IEnumerable<IEnumerable<T>> Permutations<T>(IReadOnlyList<T> items)
    {
        if (items.Count <= 1)
        {
            yield return items;
            yield break;
        }
        for (int i = 0; i < items.Count; i++)
        {
            T first = items[i];
            List<T> rest = [.. items.Take(i), .. items.Skip(i + 1)];
            foreach (IEnumerable<T> tail in Permutations(rest))
            {
                yield return [first, .. tail];
            }
        }
    }
}
