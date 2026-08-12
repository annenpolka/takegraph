using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace TakeGraph.Ymm4Bridge;

/// <summary>
/// Pins the existing directory namespace used by a render. File content leases alone are
/// insufficient because a child process resolves its arguments by pathname after launch.
/// </summary>
internal sealed class RenderPathNamespaceLease : IDisposable
{
    private const uint OpenExisting = 3;
    private const uint FileFlagBackupSemantics = 0x02000000;
    private const uint FileFlagOpenReparsePoint = 0x00200000;
    private readonly IReadOnlyList<(string Path, SafeFileHandle Handle)> directories;
    private bool disposed;

    private RenderPathNamespaceLease(
        IEnumerable<string> filePaths,
        IEnumerable<string> directoryPaths)
    {
        var files = filePaths.Select(Path.GetFullPath).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();
        var explicitDirectories = directoryPaths
            .Select(Path.GetFullPath)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .ToArray();
        foreach (var file in files)
        {
            RequireRegularFilePath(file, mustExist: true);
        }
        foreach (var directory in explicitDirectories)
        {
            RequireDirectoryPath(directory);
        }

        var paths = files
            .Select(path => Path.GetDirectoryName(path)
                ?? throw new BridgeValidationException($"Render file has no parent directory: {path}"))
            .Concat(explicitDirectories)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .OrderBy(path => path.Length)
            .ThenBy(path => path, StringComparer.OrdinalIgnoreCase)
            .ToArray();
        var opened = new List<(string, SafeFileHandle)>();
        try
        {
            foreach (var path in paths)
            {
                RequireDirectoryPath(path);
                var handle = CreateFileW(
                    path,
                    0,
                    FileShare.Read | FileShare.Write,
                    IntPtr.Zero,
                    OpenExisting,
                    FileFlagBackupSemantics | FileFlagOpenReparsePoint,
                    IntPtr.Zero);
                if (handle.IsInvalid)
                {
                    throw new IOException(
                        $"Could not lease render directory namespace: {path}",
                        new Win32Exception(Marshal.GetLastWin32Error()));
                }
                VerifyHandlePath(handle, path, "render directory");
                opened.Add((path, handle));
            }
            directories = opened;
            Verify();
        }
        catch
        {
            foreach (var (_, handle) in opened)
            {
                handle.Dispose();
            }
            throw;
        }
    }

    internal static RenderPathNamespaceLease Open(
        IEnumerable<string> filePaths,
        IEnumerable<string> directoryPaths) => new(filePaths, directoryPaths);

    internal void Verify()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        foreach (var (path, handle) in directories)
        {
            RequireDirectoryPath(path);
            VerifyHandlePath(handle, path, "render directory");
        }
    }

    internal static void RequireRegularFilePath(string path, bool mustExist)
    {
        var fullPath = Path.GetFullPath(path);
        RequireNoReparseComponents(fullPath, includeLeaf: true);
        if (Directory.Exists(fullPath))
        {
            throw new BridgeValidationException($"Render file path is a directory: {fullPath}");
        }
        if (mustExist && !File.Exists(fullPath))
        {
            throw new BridgeValidationException($"Render file is unavailable: {fullPath}");
        }
    }

    internal static void RequireDirectoryPath(string path)
    {
        var fullPath = Path.GetFullPath(path);
        if (!Directory.Exists(fullPath))
        {
            throw new BridgeValidationException($"Render directory is unavailable: {fullPath}");
        }
        RequireNoReparseComponents(fullPath, includeLeaf: true);
    }

    internal static void VerifyFileHandlePath(FileStream stream, string expectedPath) =>
        VerifyHandlePath(stream.SafeFileHandle, Path.GetFullPath(expectedPath), "render file");

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }
        disposed = true;
        foreach (var (_, handle) in directories.Reverse())
        {
            handle.Dispose();
        }
    }

    private static void RequireNoReparseComponents(string path, bool includeLeaf)
    {
        var root = Path.GetPathRoot(path)
            ?? throw new BridgeValidationException($"Render path has no root: {path}");
        var components = Path.GetRelativePath(root, path)
            .Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries);
        var current = root;
        for (var index = 0; index < components.Length; index++)
        {
            current = Path.Combine(current, components[index]);
            if (!includeLeaf && index == components.Length - 1)
            {
                break;
            }
            if (!File.Exists(current) && !Directory.Exists(current))
            {
                continue;
            }
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
            {
                throw new BridgeValidationException(
                    $"Render paths and their ancestors must not be reparse points: {current}");
            }
        }
    }

    private static void VerifyHandlePath(SafeFileHandle handle, string expectedPath, string label)
    {
        var capacity = 512;
        while (capacity <= 32_768)
        {
            var buffer = new char[capacity];
            var length = GetFinalPathNameByHandleW(handle, buffer, (uint)buffer.Length, 0);
            if (length == 0)
            {
                throw new IOException(
                    $"Could not resolve leased {label}: {expectedPath}",
                    new Win32Exception(Marshal.GetLastWin32Error()));
            }
            if (length < buffer.Length)
            {
                var actual = NormalizeHandlePath(new string(buffer, 0, checked((int)length)));
                if (!string.Equals(actual, Path.GetFullPath(expectedPath), StringComparison.OrdinalIgnoreCase))
                {
                    throw new RenderSourceDriftException(
                        $"Leased {label} resolved to a different filesystem object: {expectedPath}");
                }
                return;
            }
            capacity = checked((int)length + 1);
        }
        throw new IOException($"Resolved {label} path is too long: {expectedPath}");
    }

    private static string NormalizeHandlePath(string value)
    {
        const string uncPrefix = @"\\?\UNC\";
        const string devicePrefix = @"\\?\";
        if (value.StartsWith(uncPrefix, StringComparison.OrdinalIgnoreCase))
        {
            return Path.GetFullPath(@"\\" + value[uncPrefix.Length..]);
        }
        if (value.StartsWith(devicePrefix, StringComparison.OrdinalIgnoreCase))
        {
            value = value[devicePrefix.Length..];
        }
        return Path.GetFullPath(value);
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string fileName,
        uint desiredAccess,
        FileShare shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandleW(
        SafeFileHandle file,
        [Out] char[] filePath,
        uint filePathLength,
        uint flags);
}
