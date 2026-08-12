using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

namespace TakeGraph.Ymm4Bridge;

internal sealed partial class Ymm4Facade
{
    private const uint GenericRead = 0x80000000;
    private const uint FileReadAttributes = 0x00000080;
    private const uint FileShareRead = 0x00000001;
    private const uint FileShareWrite = 0x00000002;
    private const uint OpenExisting = 3;
    private const uint FileFlagOpenReparsePoint = 0x00200000;
    private const uint FileFlagBackupSemantics = 0x02000000;
    private const uint FileFlagSequentialScan = 0x08000000;

    private static PortableArtifactLease MaterializePortableArtifacts(
        IReadOnlyList<ManagedUtteranceDto> utterances)
    {
        var root = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "TakeGraph",
            "portable-audio-artifacts",
            "sha256");
        return MaterializePortableArtifacts(utterances, root);
    }

    internal static PortableArtifactLease MaterializePortableArtifactsForTests(
        IReadOnlyList<ManagedUtteranceDto> utterances,
        string root) => MaterializePortableArtifacts(utterances, root);

    internal static string ReadPortableArtifactHash(string path)
    {
        var fullPath = Path.GetFullPath(path);
        var namespaceLocks = OpenPortableDirectoryChain(
            Path.GetDirectoryName(fullPath)!,
            "portable artifact read-back",
            createMissing: false,
            boundary: Path.GetDirectoryName(fullPath)!);
        SafeFileHandle? handle = null;
        FileStream? stream = null;
        try
        {
            handle = CreateFileW(
                fullPath,
                GenericRead | FileReadAttributes,
                FileShareRead,
                IntPtr.Zero,
                OpenExisting,
                FileFlagOpenReparsePoint | FileFlagSequentialScan,
                IntPtr.Zero);
            EnsureValidHandle(handle, fullPath);
            var information = ReadPortableHandleInformation(handle, fullPath);
            if ((information.FileAttributes & FileAttributes.ReparsePoint) != 0
                || (information.FileAttributes & FileAttributes.Directory) != 0)
            {
                throw new BridgeValidationException(
                    $"Portable artifact read-back must be a regular, non-reparse file: {fullPath}");
            }
            stream = new FileStream(handle, FileAccess.Read, 128 * 1024, isAsync: false);
            handle = null;
            var hash = Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
            var current = ReadPortableHandleInformation(stream.SafeFileHandle, fullPath);
            if (current.VolumeSerialNumber != information.VolumeSerialNumber
                || current.FileIndexHigh != information.FileIndexHigh
                || current.FileIndexLow != information.FileIndexLow
                || current.FileSizeHigh != information.FileSizeHigh
                || current.FileSizeLow != information.FileSizeLow)
            {
                throw new BridgeValidationException(
                    $"Portable artifact changed during read-back: {fullPath}");
            }
            return hash;
        }
        finally
        {
            stream?.Dispose();
            handle?.Dispose();
            DisposeHandles(namespaceLocks);
        }
    }

    private static PortableArtifactLease MaterializePortableArtifacts(
        IReadOnlyList<ManagedUtteranceDto> utterances,
        string root)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(root);
        var leases = new Dictionary<string, PortableLockedFile>(StringComparer.OrdinalIgnoreCase);
        try
        {
            var materialized = new List<ManagedUtteranceDto>(utterances.Count);
            foreach (var utterance in utterances)
            {
                var hash = NormalizePortableArtifactHash(utterance.ArtifactHash);
                if (!leases.TryGetValue(hash, out var lease))
                {
                    lease = OpenOrCreatePortableCasFile(root, utterance.AudioPath, hash);
                    leases.Add(hash, lease);
                }
                materialized.Add(utterance with { AudioPath = lease.Path });
            }
            return new PortableArtifactLease(materialized, leases);
        }
        catch
        {
            foreach (var lease in leases.Values)
            {
                lease.Dispose();
            }
            throw;
        }
    }

    private static PortableLockedFile OpenOrCreatePortableCasFile(
        string root,
        string sourcePath,
        string expectedHash)
    {
        var fullRoot = Path.GetFullPath(root);
        var directory = Path.Combine(fullRoot, expectedHash[..2]);
        var namespaceLocks = OpenPortableDirectoryChain(
            directory,
            "portable artifact directory",
            createMissing: true,
            boundary: fullRoot);
        var destination = Path.Combine(directory, $"{expectedHash}.wav");
        try
        {
            if (File.Exists(destination))
            {
                return OpenVerifiedPortableArtifact(destination, expectedHash, namespaceLocks);
            }

            using var input = OpenVerifiedPortableArtifact(
                Path.GetFullPath(sourcePath),
                expectedHash,
                OpenPortableDirectoryChain(
                    Path.GetDirectoryName(Path.GetFullPath(sourcePath))!,
                    "portable artifact source",
                    createMissing: false,
                    boundary: Path.GetDirectoryName(Path.GetFullPath(sourcePath))!));
            var temporary = Path.Combine(directory, $".{expectedHash}.{Guid.NewGuid():N}.tmp");
            try
            {
                using (var output = new FileStream(
                           temporary,
                           FileMode.CreateNew,
                           FileAccess.ReadWrite,
                           FileShare.None,
                           128 * 1024,
                           FileOptions.WriteThrough | FileOptions.SequentialScan))
                {
                    input.Stream.CopyTo(output);
                    output.Flush(flushToDisk: true);
                    output.Position = 0;
                    VerifyStreamHash(output, expectedHash, temporary);
                }
                try
                {
                    File.Move(temporary, destination, overwrite: false);
                }
                catch (IOException) when (File.Exists(destination))
                {
                    // A racing publisher is acceptable only if the no-follow
                    // handle below proves it contains the same approved bytes.
                }
                if (File.Exists(destination))
                {
                    File.SetAttributes(
                        destination,
                        File.GetAttributes(destination) | FileAttributes.ReadOnly);
                }
                return OpenVerifiedPortableArtifact(destination, expectedHash, namespaceLocks);
            }
            finally
            {
                if (File.Exists(temporary))
                {
                    File.Delete(temporary);
                }
            }
        }
        catch
        {
            DisposeHandles(namespaceLocks);
            throw;
        }
    }

    private static PortableLockedFile OpenVerifiedPortableArtifact(
        string path,
        string expectedHash,
        IReadOnlyList<SafeFileHandle> namespaceLocks)
    {
        var fullPath = Path.GetFullPath(path);
        SafeFileHandle? handle = null;
        FileStream? stream = null;
        try
        {
            handle = CreateFileW(
                fullPath,
                GenericRead | FileReadAttributes,
                FileShareRead,
                IntPtr.Zero,
                OpenExisting,
                FileFlagOpenReparsePoint | FileFlagSequentialScan,
                IntPtr.Zero);
            EnsureValidHandle(handle, fullPath);
            var information = ReadPortableHandleInformation(handle, fullPath);
            if ((information.FileAttributes & FileAttributes.ReparsePoint) != 0
                || (information.FileAttributes & FileAttributes.Directory) != 0)
            {
                throw new BridgeValidationException(
                    $"Portable artifact must be a regular, non-reparse file: {fullPath}");
            }
            stream = new FileStream(handle, FileAccess.Read, 128 * 1024, isAsync: false);
            handle = null; // FileStream owns it now.
            VerifyStreamHash(stream, expectedHash, fullPath);
            return new PortableLockedFile(fullPath, stream, namespaceLocks, information);
        }
        catch
        {
            stream?.Dispose();
            handle?.Dispose();
            DisposeHandles(namespaceLocks);
            throw;
        }
    }

    private static IReadOnlyList<SafeFileHandle> OpenPortableDirectoryChain(
        string directory,
        string label,
        bool createMissing,
        string boundary)
    {
        var fullPath = Path.GetFullPath(directory);
        var root = Path.GetFullPath(boundary);
        var relative = Path.GetRelativePath(root, fullPath);
        if (relative == ".." || relative.StartsWith($"..{Path.DirectorySeparatorChar}", StringComparison.Ordinal)
            || Path.IsPathRooted(relative))
        {
            throw new BridgeValidationException($"{label} escapes its pinned namespace boundary");
        }
        var handles = new List<SafeFileHandle>();
        var current = root;
        try
        {
            var components = relative == "."
                ? Array.Empty<string>()
                : relative.Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries);
            foreach (var component in new[] { string.Empty }.Concat(components))
            {
                if (component.Length > 0)
                {
                    current = Path.Combine(current, component);
                }
                if (!Directory.Exists(current))
                {
                    if (!createMissing)
                    {
                        throw new BridgeValidationException(
                            $"{label} directory does not exist: {current}");
                    }
                    Directory.CreateDirectory(current);
                }
                var handle = CreateFileW(
                    current,
                    0,
                    FileShareRead | FileShareWrite,
                    IntPtr.Zero,
                    OpenExisting,
                    FileFlagOpenReparsePoint | FileFlagBackupSemantics,
                    IntPtr.Zero);
                EnsureValidHandle(handle, current);
                var information = ReadPortableHandleInformation(handle, current);
                if ((information.FileAttributes & FileAttributes.ReparsePoint) != 0
                    || (information.FileAttributes & FileAttributes.Directory) == 0)
                {
                    handle.Dispose();
                    throw new BridgeValidationException(
                        $"{label} contains a non-directory or reparse component: {current}");
                }
                handles.Add(handle);
            }
            return handles;
        }
        catch
        {
            DisposeHandles(handles);
            throw;
        }
    }

    private static PortableFileInformation ReadPortableHandleInformation(
        SafeFileHandle handle,
        string path)
    {
        if (!GetFileInformationByHandle(handle, out var information))
        {
            throw new BridgeValidationException(
                $"Unable to inspect portable artifact handle {path}: {LastWin32Error()}");
        }
        return information;
    }

    private static void EnsureValidHandle(SafeFileHandle handle, string path)
    {
        if (handle.IsInvalid)
        {
            var error = LastWin32Error();
            handle.Dispose();
            throw new BridgeValidationException(
                $"Unable to open portable artifact path {path}: {error}");
        }
    }

    private static string LastWin32Error() =>
        new Win32Exception(Marshal.GetLastPInvokeError()).Message;

    private static void DisposeHandles(IEnumerable<SafeFileHandle> handles)
    {
        foreach (var handle in handles)
        {
            handle.Dispose();
        }
    }

    private static void VerifyStreamHash(FileStream stream, string expectedHash, string path)
    {
        stream.Position = 0;
        var actual = Convert.ToHexString(SHA256.HashData(stream)).ToLowerInvariant();
        stream.Position = 0;
        if (!string.Equals(actual, expectedHash, StringComparison.Ordinal))
        {
            throw new BridgeValidationException(
                $"Portable audio artifact hash mismatch: {Path.GetFullPath(path)}");
        }
    }

    private static string NormalizePortableArtifactHash(string value)
    {
        var hash = value.Trim().ToLowerInvariant();
        if (hash.Length != 64 || hash.Any(character => !Uri.IsHexDigit(character)))
        {
            throw new BridgeValidationException(
                "Portable audio artifactHash must be a 64-character SHA-256 hex digest");
        }
        return hash;
    }

    internal sealed class PortableArtifactLease : IDisposable
    {
        private readonly IReadOnlyDictionary<string, PortableLockedFile> leases;

        internal PortableArtifactLease(
            IReadOnlyList<ManagedUtteranceDto> utterances,
            IReadOnlyDictionary<string, PortableLockedFile> leases)
        {
            Utterances = utterances;
            this.leases = leases;
        }

        internal IReadOnlyList<ManagedUtteranceDto> Utterances { get; }

        internal void Verify()
        {
            foreach (var (hash, lease) in leases)
            {
                var current = ReadPortableHandleInformation(lease.Stream.SafeFileHandle, lease.Path);
                if (current.VolumeSerialNumber != lease.Information.VolumeSerialNumber
                    || current.FileIndexHigh != lease.Information.FileIndexHigh
                    || current.FileIndexLow != lease.Information.FileIndexLow
                    || current.FileSizeHigh != lease.Information.FileSizeHigh
                    || current.FileSizeLow != lease.Information.FileSizeLow)
                {
                    throw new BridgeValidationException(
                        $"Portable artifact file identity changed during apply: {lease.Path}");
                }
                VerifyStreamHash(lease.Stream, hash, lease.Path);
            }
        }

        public void Dispose()
        {
            foreach (var lease in leases.Values)
            {
                lease.Dispose();
            }
        }
    }

    internal sealed class PortableLockedFile : IDisposable
    {
        private readonly IReadOnlyList<SafeFileHandle> namespaceLocks;

        internal PortableLockedFile(
            string path,
            FileStream stream,
            IReadOnlyList<SafeFileHandle> namespaceLocks,
            PortableFileInformation information)
        {
            Path = path;
            Stream = stream;
            this.namespaceLocks = namespaceLocks;
            Information = information;
        }

        internal string Path { get; }
        internal FileStream Stream { get; }
        internal PortableFileInformation Information { get; }

        public void Dispose()
        {
            Stream.Dispose();
            DisposeHandles(namespaceLocks);
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct PortableFileInformation
    {
        internal FileAttributes FileAttributes;
        private readonly System.Runtime.InteropServices.ComTypes.FILETIME creationTime;
        private readonly System.Runtime.InteropServices.ComTypes.FILETIME lastAccessTime;
        private readonly System.Runtime.InteropServices.ComTypes.FILETIME lastWriteTime;
        internal uint VolumeSerialNumber;
        internal uint FileSizeHigh;
        internal uint FileSizeLow;
        private readonly uint numberOfLinks;
        internal uint FileIndexHigh;
        internal uint FileIndexLow;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetFileInformationByHandle(
        SafeFileHandle file,
        out PortableFileInformation fileInformation);
}
