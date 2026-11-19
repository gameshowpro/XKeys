// Copyright (c) Barjonas LLC 2026.
using System.Diagnostics.CodeAnalysis;
using System.IO;
using System.Runtime.InteropServices;

namespace GameshowPro.XKeys.Hid;

internal static unsafe partial class XkHid
{
    /// <summary>Every export the binding imports, checked for before any is called.</summary>
    internal static readonly string[] ExportNames =
    [
        "xkhid_abi_version", "xkhid_entry_size", "xkhid_info_size", "xkhid_timestamp_offset", "xkhid_enumerate", "xkhid_open",
        "xkhid_close", "xkhid_event_handle", "xkhid_read", "xkhid_dropped", "xkhid_write", "xkhid_input_len", "xkhid_output_len",
        "xkhid_watch_create", "xkhid_watch_event_handle", "xkhid_watch_generation", "xkhid_watch_destroy",
    ];

    private static readonly Lock s_loadLock = new();
    private static bool s_attempted;
    private static string? s_unavailableReason;
    private static nint s_library;

    /// <summary>
    /// Load <c>xkhid.dll</c> if it is not loaded yet and check that it matches this binding (every export, ABI version, entry and
    /// info sizes). Returns false, with a reason that names the DLL, when it is missing or does not match. The answer is the same
    /// for the rest of the process.
    /// </summary>
    public static bool TryLoad([NotNullWhen(false)] out string? unavailableReason)
    {
        lock (s_loadLock)
        {
            if (!s_attempted)
            {
                s_attempted = true;
                s_unavailableReason = Load(DefaultSearchDirectories());
            }
            unavailableReason = s_unavailableReason;
            return unavailableReason is null;
        }
    }

    /// <summary>As <see cref="TryLoad"/>, but throws an <see cref="XkHidException"/> that names the DLL when it cannot be used (fail fast).</summary>
    public static void Require()
    {
        if (!TryLoad(out string? reason))
        {
            throw new XkHidException(reason);
        }
    }

    /// <summary>Where the DLL is looked for, in order: next to this assembly, then in the NuGet native asset folder beside it, then the same two under the application base directory.</summary>
    internal static IEnumerable<string> DefaultSearchDirectories()
    {
        string? assemblyDirectory = Path.GetDirectoryName(typeof(XkHid).Assembly.Location);
        foreach (string? root in new[] { assemblyDirectory, AppContext.BaseDirectory })
        {
            if (!string.IsNullOrEmpty(root))
            {
                yield return root;
                yield return Path.Combine(root, "runtimes", "win-x64", "native");
            }
        }
    }

    /// <summary>
    /// Load and check the DLL from <paramref name="directories"/>, and, if it matches, make it the target of the generated imports.
    /// Returns null on success, else the reason, which names the DLL. Only <see cref="TryLoad"/> may call it with a directory list that
    /// can succeed, because the runtime allows the import resolver to be set once; tests call it with lists that cannot.
    /// </summary>
    internal static string? Load(IEnumerable<string> directories)
    {
        if (!OperatingSystem.IsWindows() || RuntimeInformation.ProcessArchitecture != Architecture.X64)
        {
            return $"{LibraryFileName} is only built for Windows x64";
        }
        nint library = TryLoadFrom(directories, out string? reason);
        if (library == 0)
        {
            return reason;
        }
        // Make the generated imports resolve to the library that was just checked, not to whatever the default search finds.
        s_library = library;
        NativeLibrary.SetDllImportResolver(typeof(XkHid).Assembly, (name, _, _) => name == LibraryName ? s_library : 0);
        string? mismatch = CheckVersions(AbiVersionExport(), EntrySizeExport(), InfoSizeExport());
        if (mismatch is not null)
        {
            s_library = 0;
            return mismatch;
        }
        // The library is never freed: the runtime keeps resolved imports for the life of the process.
        return null;
    }

    /// <summary>
    /// Load the first <c>xkhid.dll</c> found in <paramref name="directories"/> and check that it has every export the binding imports.
    /// Returns its handle, or 0 with a reason (which names the DLL) when none loads or one lacks an export; a library that fails is freed.
    /// Nothing in it is called.
    /// </summary>
    internal static nint TryLoadFrom(IEnumerable<string> directories, [NotNullWhen(false)] out string? reason)
    {
        string? firstFailure = null;
        string? searched = null;
        foreach (string directory in directories)
        {
            string path = Path.Combine(directory, LibraryFileName);
            searched = searched is null ? directory : $"{searched}, {directory}";
            if (!File.Exists(path))
            {
                continue;
            }
            try
            {
                if (!NativeLibrary.TryLoad(path, out nint library))
                {
                    firstFailure ??= $"{path} could not be loaded";
                    continue;
                }
                foreach (string export in ExportNames)
                {
                    if (!NativeLibrary.TryGetExport(library, export, out _))
                    {
                        NativeLibrary.Free(library);
                        reason = $"{path} has no export {export}";
                        return 0;
                    }
                }
                reason = null;
                return library;
            }
            catch (Exception ex) when (ex is BadImageFormatException or ArgumentException or DllNotFoundException)
            {
                firstFailure ??= $"{path} could not be loaded: {ex.Message}";
            }
        }
        reason = firstFailure ?? $"{LibraryFileName} was not found (looked in {searched ?? "no directories"}); the library build copies it from the Rust crate, which needs cargo";
        return 0;
    }

    /// <summary>Compare what the DLL reports with what this binding was written for. Returns null if they agree, else a reason that names the DLL.</summary>
    internal static string? CheckVersions(uint abiVersion, uint entrySize, uint infoSize)
    {
        if (abiVersion != AbiVersion)
        {
            return $"{LibraryFileName} has ABI version {abiVersion}, but this build expects {AbiVersion}";
        }
        if (entrySize != XkReportEntry.Size || sizeof(XkReportEntry) != XkReportEntry.Size)
        {
            return $"{LibraryFileName} report entries are {entrySize} bytes, but this build's are {sizeof(XkReportEntry)}";
        }
        if (infoSize != XkDeviceInfo.Size || sizeof(XkDeviceInfo) != XkDeviceInfo.Size)
        {
            return $"{LibraryFileName} device info structures are {infoSize} bytes, but this build's are {sizeof(XkDeviceInfo)}";
        }
        return null;
    }
}
