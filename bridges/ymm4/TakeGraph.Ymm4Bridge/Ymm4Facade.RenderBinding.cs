using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed record RenderBindingFile(
    string Role,
    string Path,
    string Sha256,
    ulong ByteLength);

internal sealed record RenderRuntimeBinding(
    string SelectorPlugin,
    string WriterPlugin,
    string VideoArgs,
    string ResolvedVideoArgs,
    string AudioArgs,
    string VideoCodec,
    string AudioCodec,
    string PixelFormat,
    string Container,
    uint WriterWidth,
    uint WriterHeight,
    uint WriterFps,
    uint WriterAudioHz,
    string ManifestDigest,
    string DriverProfileDigest,
    IReadOnlyList<RenderBindingFile> Files)
{
    internal RenderBindingLease OpenVerifiedReadLease() => new(this);
}

internal sealed record BoundRenderProfile(
    RenderProfileDescriptorDto Profile,
    RenderRuntimeBinding Binding);

internal sealed class RenderBindingLease : IDisposable
{
    private readonly IReadOnlyList<(RenderBindingFile Evidence, FileStream Stream)> files;
    private readonly RenderPathNamespaceLease namespaceLease;
    private bool disposed;

    internal RenderBindingLease(RenderRuntimeBinding binding)
    {
        var opened = new List<(RenderBindingFile, FileStream)>();
        RenderPathNamespaceLease? openedNamespace = null;
        try
        {
            openedNamespace = RenderPathNamespaceLease.Open(
                binding.Files.Select(value => value.Path),
                []);
            foreach (var evidence in binding.Files)
            {
                var stream = new FileStream(
                    evidence.Path,
                    FileMode.Open,
                    FileAccess.Read,
                    FileShare.Read,
                    64 * 1024,
                    FileOptions.SequentialScan);
                RenderPathNamespaceLease.VerifyFileHandlePath(stream, evidence.Path);
                opened.Add((evidence, stream));
                VerifyOne(evidence, stream);
            }
            files = opened;
            namespaceLease = openedNamespace;
        }
        catch
        {
            foreach (var (_, stream) in opened)
            {
                stream.Dispose();
            }
            openedNamespace?.Dispose();
            throw;
        }
    }

    internal void Verify()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        namespaceLease.Verify();
        foreach (var (evidence, stream) in files)
        {
            RenderPathNamespaceLease.RequireRegularFilePath(evidence.Path, mustExist: true);
            RenderPathNamespaceLease.VerifyFileHandlePath(stream, evidence.Path);
            VerifyOne(evidence, stream);
        }
    }

    public void Dispose()
    {
        if (disposed)
        {
            return;
        }
        disposed = true;
        foreach (var (_, stream) in files)
        {
            stream.Dispose();
        }
        namespaceLease.Dispose();
    }

    private static void VerifyOne(RenderBindingFile evidence, FileStream stream)
    {
        if (checked((ulong)stream.Length) != evidence.ByteLength)
        {
            throw new RenderSourceDriftException(
                $"Render binding file length changed: {evidence.Role}");
        }
        stream.Position = 0;
        var digest = Convert.ToHexStringLower(SHA256.HashData(stream));
        stream.Position = 0;
        if (!string.Equals(digest, evidence.Sha256, StringComparison.Ordinal))
        {
            throw new RenderSourceDriftException(
                $"Render binding file hash changed: {evidence.Role}");
        }
    }
}

internal sealed partial class Ymm4Facade
{
    internal const string AuthoritativeRenderSourceBindingError =
        "Authoritative render is unavailable because YMM4 4.55.1.1 does not expose a proven "
        + "exhaustive render dependency manifest for project assets, fonts, voice engines, "
        + "tachie/effect inputs, and plugin settings; every child-encoder input must be "
        + "SHA-256-bound and read-leased before project.render can be advertised";

    private const string FfmpegWriterPluginType =
        "YukkuriMovieMaker.Plugin.FileSource.FFmpeg.FFmpegVideoFileWriterPlugin";
    private const string FfmpegWriterSettingsType =
        "YukkuriMovieMaker.Plugin.FileSource.FFmpeg.FFmpegVideoFileWriterSettings";
    private const string WriterSelectorSettingsType =
        "YukkuriMovieMaker.VideoFileWriter.VideoFileWriterSettings";
    private const string CommandLineEncoderType =
        "YukkuriMovieMaker.CommandLine.CommandLineEncoder";

    private static bool TryCaptureRenderRuntimeBinding(
        out RenderRuntimeBinding? binding,
        out string error)
    {
        try
        {
            binding = CaptureRenderRuntimeBinding();
            RequireAuthoritativeRenderSourceBinding();
            error = string.Empty;
            return true;
        }
        catch (Exception exception)
        {
            binding = null;
            error = exception.GetBaseException().Message;
            return false;
        }
    }

    internal static void RequireAuthoritativeRenderSourceBinding()
    {
        // TimelineResourceList is a UI-oriented inventory. No supported 4.55.1.1
        // contract proves that it exhaustively covers every file/configuration the
        // command-line encoder can resolve. Advertising a render profile before we
        // can seal and lease that complete closure would let same-length/mtime asset
        // substitutions escape the snapshot/profile digest.
        throw new BridgeUnavailableException(AuthoritativeRenderSourceBindingError);
    }

    private static RenderRuntimeBinding CaptureRenderRuntimeBinding()
    {
        var selectorType = RequireLoadedRenderType(WriterSelectorSettingsType);
        var selector = ReadSettingsDefault(selectorType);
        var selectorPath = ResolveLoadedSettingsPath(selectorType, selector);
        using var selectorJson = ReadJsonFile(selectorPath);
        var selectorPlugin = RequireStringMember(selector, "Plugin");
        RequireJsonScalar(selectorJson.RootElement, "Plugin", selectorPlugin);
        if (!string.Equals(selectorPlugin, FfmpegWriterPluginType, StringComparison.Ordinal))
        {
            throw new BridgeUnavailableException(
                $"Unsupported active YMM4 writer: {selectorPlugin}");
        }

        var encoderType = RequireLoadedRenderType(CommandLineEncoderType);
        var resolvePlugin = encoderType.GetMethod(
            "ResolvePlugin",
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new BridgeUnavailableException(
                "YMM4 command-line writer resolution is unavailable");
        object selectedPlugin;
        try
        {
            selectedPlugin = resolvePlugin.Invoke(null, null)
                ?? throw new BridgeUnavailableException(
                    "YMM4 command-line writer resolution returned no writer");
        }
        catch (TargetInvocationException exception)
        {
            throw exception.InnerException ?? exception;
        }
        var writerPlugin = selectedPlugin.GetType().FullName ?? string.Empty;
        if (!string.Equals(writerPlugin, selectorPlugin, StringComparison.Ordinal)
            || !string.Equals(writerPlugin, FfmpegWriterPluginType, StringComparison.Ordinal))
        {
            throw new BridgeUnavailableException(
                "The command-line encoder did not resolve the active writer selector exactly");
        }

        var writerSettingsType = RequireLoadedRenderType(FfmpegWriterSettingsType);
        var writerSettings = ReadSettingsDefault(writerSettingsType);
        var writerSettingsPath = ResolveLoadedSettingsPath(writerSettingsType, writerSettings);
        using var writerJson = ReadJsonFile(writerSettingsPath);
        foreach (var property in new[]
                 {
                     "Width", "Height", "FPS", "Hz", "Length", "VideoBitRateMode",
                     "VideoBitRateControlMode", "VideoBitRate", "VideoQuality", "AudioBitRate",
                     "VideoOptions", "AudioOptions", "IsHardwareEncoderEnabled", "PresetVersion",
                 })
        {
            RequireJsonScalar(
                writerJson.RootElement,
                property,
                ReadRequiredMember(writerSettings, property)?.ToString() ?? string.Empty);
        }
        if (ReadRequiredMember(writerSettings, "HasErrors") is true)
        {
            throw new BridgeUnavailableException("The active FFmpeg writer settings contain errors");
        }

        var videoArgs = RequireStringMember(writerSettings, "VideoArgsString");
        var resolvedVideoArgs = RequireStringMember(writerSettings, "ResolvedVideoArgsString");
        var audioArgs = RequireStringMember(writerSettings, "AudioArgsString");
        var writerWidth = RequirePositiveUInt(writerSettings, "Width");
        var writerHeight = RequirePositiveUInt(writerSettings, "Height");
        var writerFps = RequirePositiveUInt(writerSettings, "FPS");
        var writerAudioHz = RequirePositiveUInt(writerSettings, "Hz");
        var videoCodec = NormalizeH264Encoder(
            RequireUniqueOption(resolvedVideoArgs, "-c:v", "-codec:v", "-vcodec"));
        var audioCodec = NormalizeAacEncoder(
            RequireUniqueOption(audioArgs, "-c:a", "-codec:a", "-acodec"));
        var pixelFormat = RequireUniqueOption(resolvedVideoArgs, "-pix_fmt");
        if (!string.Equals(pixelFormat, "yuv420p", StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeUnavailableException(
                $"Unsupported FFmpeg pixel format: {pixelFormat}");
        }
        var videoContainer = RequireUniqueOption(resolvedVideoArgs, "-f");
        var audioContainer = RequireUniqueOption(audioArgs, "-f");
        if (!string.Equals(videoContainer, "mp4", StringComparison.OrdinalIgnoreCase)
            || !string.Equals(audioContainer, "mp4", StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeUnavailableException(
                "The active FFmpeg video/audio muxer is not exactly MP4");
        }

        var getFfmpegPath = selectedPlugin.GetType().GetMethod(
            "GetFFmpegExePath",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new BridgeUnavailableException("The FFmpeg binary resolver is unavailable");
        string ffmpegPath;
        try
        {
            ffmpegPath = Path.GetFullPath(
                getFfmpegPath.Invoke(selectedPlugin, null)?.ToString()
                ?? throw new BridgeUnavailableException("The FFmpeg binary path is unavailable"));
        }
        catch (TargetInvocationException exception)
        {
            throw exception.InnerException ?? exception;
        }
        if (!File.Exists(ffmpegPath)
            || !string.Equals(Path.GetFileName(ffmpegPath), "ffmpeg.exe", StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeUnavailableException("The selected FFmpeg executable is unavailable");
        }

        var processPath = Environment.ProcessPath;
        if (string.IsNullOrWhiteSpace(processPath) || !File.Exists(processPath))
        {
            throw new BridgeUnavailableException("The YMM4 executable path is unavailable");
        }
        processPath = Path.GetFullPath(processPath);
        if (!string.Equals(
                Path.GetFileName(processPath),
                "YukkuriMovieMaker.exe",
                StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeUnavailableException("The render binder is not running inside YMM4");
        }

        var files = new List<(string Role, string Path)>
        {
            ("writer-selector-settings", selectorPath),
            ("writer-settings", writerSettingsPath),
            ("ymm4-executable", processPath),
            ("command-line-encoder-assembly", encoderType.Assembly.Location),
            ("writer-selector-assembly", selectorType.Assembly.Location),
            ("writer-plugin-assembly", selectedPlugin.GetType().Assembly.Location),
            ("ffmpeg-executable", ffmpegPath),
        };
        var applicationDirectory = Path.GetDirectoryName(processPath)
            ?? throw new BridgeUnavailableException("The YMM4 application directory is unavailable");
        foreach (var dependency in new[]
                 {
                     "FFmpeg.AutoGen.dll",
                     "YukkuriMovieMaker.Interop.FFmpeg.dll",
                     "YukkuriMovieMaker.Plugin.dll",
                 })
        {
            var dependencyPath = Path.Combine(applicationDirectory, dependency);
            if (!File.Exists(dependencyPath))
            {
                throw new BridgeUnavailableException(
                    $"Required writer dependency is unavailable: {dependency}");
            }
            files.Add(($"writer-dependency:{dependency}", dependencyPath));
        }
        var ffmpegDirectory = Path.GetDirectoryName(ffmpegPath)
            ?? throw new BridgeUnavailableException("The FFmpeg binary directory is unavailable");
        var ffmpegLibraries = Directory.EnumerateFiles(ffmpegDirectory, "*.dll")
            .OrderBy(path => Path.GetFileName(path), StringComparer.OrdinalIgnoreCase)
            .ToArray();
        foreach (var prefix in new[]
                 {
                     "avcodec-", "avdevice-", "avfilter-", "avformat-", "avutil-",
                     "swresample-", "swscale-",
                 })
        {
            if (ffmpegLibraries.Count(path => Path.GetFileName(path).StartsWith(
                    prefix,
                    StringComparison.OrdinalIgnoreCase)) != 1)
            {
                throw new BridgeUnavailableException(
                    $"The FFmpeg dependency set is unsupported: {prefix}");
            }
        }
        files.AddRange(ffmpegLibraries.Select(path => ($"ffmpeg-library:{Path.GetFileName(path)}", path)));

        var evidencePaths = files
            .Select(value => (value.Role, Path: Path.GetFullPath(value.Path)))
            .DistinctBy(value => value.Path, StringComparer.OrdinalIgnoreCase)
            .OrderBy(value => value.Role, StringComparer.Ordinal)
            .ToArray();
        using var captureNamespaceLease = RenderPathNamespaceLease.Open(
            evidencePaths.Select(value => value.Path),
            []);
        var evidence = evidencePaths
            .Select(value =>
            {
                var measured = ReadFileEvidence(value.Path);
                return new RenderBindingFile(
                    value.Role,
                    value.Path,
                    measured.Sha256,
                    measured.ByteLength);
            })
            .ToArray();
        var driverFields = new SortedDictionary<string, string>(StringComparer.Ordinal)
        {
            ["driver"] = RenderDriver,
            ["selectorPlugin"] = selectorPlugin,
            ["writerPlugin"] = writerPlugin,
        };
        foreach (var file in evidence.Where(value =>
                     value.Role is not ("writer-selector-settings" or "writer-settings")))
        {
            driverFields[$"{file.Role}:path"] = file.Path;
            driverFields[$"{file.Role}:sha256"] = file.Sha256;
            driverFields[$"{file.Role}:bytes"] = file.ByteLength.ToString(
                System.Globalization.CultureInfo.InvariantCulture);
        }
        var driverDigest = HashDescriptor("render-driver-v2", driverFields);
        var manifestFields = new SortedDictionary<string, string>(driverFields, StringComparer.Ordinal)
        {
            ["videoArgs"] = videoArgs,
            ["resolvedVideoArgs"] = resolvedVideoArgs,
            ["audioArgs"] = audioArgs,
            ["videoCodec"] = videoCodec,
            ["audioCodec"] = audioCodec,
            ["pixelFormat"] = pixelFormat.ToLowerInvariant(),
            ["container"] = "mp4",
            ["writerWidth"] = writerWidth.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["writerHeight"] = writerHeight.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["writerFps"] = writerFps.ToString(System.Globalization.CultureInfo.InvariantCulture),
            ["writerAudioHz"] = writerAudioHz.ToString(System.Globalization.CultureInfo.InvariantCulture),
        };
        foreach (var file in evidence.Where(value =>
                     value.Role is "writer-selector-settings" or "writer-settings"))
        {
            manifestFields[$"{file.Role}:path"] = file.Path;
            manifestFields[$"{file.Role}:sha256"] = file.Sha256;
            manifestFields[$"{file.Role}:bytes"] = file.ByteLength.ToString(
                System.Globalization.CultureInfo.InvariantCulture);
        }
        return new RenderRuntimeBinding(
            selectorPlugin,
            writerPlugin,
            videoArgs,
            resolvedVideoArgs,
            audioArgs,
            videoCodec,
            audioCodec,
            pixelFormat.ToLowerInvariant(),
            "mp4",
            writerWidth,
            writerHeight,
            writerFps,
            writerAudioHz,
            HashDescriptor("render-binding-v1", manifestFields),
            driverDigest,
            evidence);
    }

    private static Type RequireLoadedRenderType(string fullName) =>
        AppDomain.CurrentDomain.GetAssemblies()
            .Select(assembly => assembly.GetType(fullName, throwOnError: false, ignoreCase: false))
            .FirstOrDefault(type => type is not null)
        ?? throw new BridgeUnavailableException($"Required YMM4 render type is unavailable: {fullName}");

    private static object ReadSettingsDefault(Type type)
    {
        var property = type.GetProperty(
            "Default",
            BindingFlags.Public | BindingFlags.Static | BindingFlags.FlattenHierarchy)
            ?? throw new BridgeUnavailableException($"YMM4 settings Default is unavailable: {type.FullName}");
        return property.GetValue(null)
            ?? throw new BridgeUnavailableException($"YMM4 settings Default is null: {type.FullName}");
    }

    private static string ResolveLoadedSettingsPath(Type type, object settings)
    {
        var baseType = type.BaseType;
        while (baseType is not null
               && (!baseType.IsGenericType
                   || baseType.GetGenericTypeDefinition().FullName
                   != "YukkuriMovieMaker.Plugin.SettingsBase`1"))
        {
            baseType = baseType.BaseType;
        }
        if (baseType is null)
        {
            throw new BridgeUnavailableException($"Unsupported YMM4 settings base: {type.FullName}");
        }
        var pathMethod = baseType.GetMethod(
            "GetSettingFilePath",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new BridgeUnavailableException("YMM4 settings path discovery is unavailable");
        var currentVersionField = baseType.GetField(
            "currentVersion",
            BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new BridgeUnavailableException("YMM4 loaded settings version is unavailable");
        if (currentVersionField.GetValue(settings) is not Version currentVersion)
        {
            throw new BridgeUnavailableException("YMM4 loaded settings version is invalid");
        }
        var candidate = pathMethod.Invoke(settings, [currentVersion])?.ToString();
        if (!string.IsNullOrWhiteSpace(candidate) && File.Exists(candidate))
        {
            return Path.GetFullPath(candidate);
        }
        throw new BridgeUnavailableException($"Loaded YMM4 settings file is unavailable: {type.FullName}");
    }

    private static JsonDocument ReadJsonFile(string path)
    {
        RenderPathNamespaceLease.RequireRegularFilePath(path, mustExist: true);
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        RenderPathNamespaceLease.VerifyFileHandlePath(stream, path);
        return JsonDocument.Parse(stream, new JsonDocumentOptions
        {
            AllowTrailingCommas = false,
            CommentHandling = JsonCommentHandling.Disallow,
            MaxDepth = 32,
        });
    }

    private static object? ReadRequiredMember(object value, string name)
    {
        var property = value.GetType().GetProperty(
            name,
            BindingFlags.Public | BindingFlags.Instance)
            ?? throw new BridgeUnavailableException($"Required writer setting is unavailable: {name}");
        return property.GetValue(value);
    }

    private static string RequireStringMember(object value, string name)
    {
        var result = ReadRequiredMember(value, name)?.ToString();
        if (string.IsNullOrWhiteSpace(result))
        {
            throw new BridgeUnavailableException($"Required writer setting is empty: {name}");
        }
        return result;
    }

    private static uint RequirePositiveUInt(object value, string name)
    {
        var text = ReadRequiredMember(value, name)?.ToString();
        if (!uint.TryParse(
                text,
                System.Globalization.NumberStyles.None,
                System.Globalization.CultureInfo.InvariantCulture,
                out var result)
            || result == 0)
        {
            throw new BridgeUnavailableException($"Required writer setting is invalid: {name}");
        }
        return result;
    }

    private static void RequireJsonScalar(JsonElement root, string name, string expected)
    {
        if (root.ValueKind != JsonValueKind.Object || !root.TryGetProperty(name, out var property))
        {
            throw new BridgeUnavailableException($"Persisted writer setting is missing: {name}");
        }
        var actual = property.ValueKind switch
        {
            JsonValueKind.String => property.GetString() ?? string.Empty,
            JsonValueKind.Number => property.GetRawText(),
            JsonValueKind.True => bool.TrueString,
            JsonValueKind.False => bool.FalseString,
            _ => throw new BridgeUnavailableException(
                $"Persisted writer setting is not scalar: {name}"),
        };
        if (!string.Equals(actual, expected, StringComparison.OrdinalIgnoreCase))
        {
            throw new BridgeUnavailableException(
                $"In-memory and child-process writer settings differ: {name}");
        }
    }

    private static string RequireUniqueOption(string arguments, params string[] names)
    {
        var tokens = TokenizeArguments(arguments);
        var matches = new List<string>();
        for (var index = 0; index < tokens.Count; index++)
        {
            if (!names.Contains(tokens[index], StringComparer.OrdinalIgnoreCase))
            {
                continue;
            }
            if (index + 1 >= tokens.Count || tokens[index + 1].StartsWith('-'))
            {
                throw new BridgeUnavailableException(
                    $"FFmpeg option has no value: {tokens[index]}");
            }
            matches.Add(tokens[index + 1]);
        }
        if (matches.Count == 0
            || matches.Select(value => value.ToLowerInvariant()).Distinct(StringComparer.Ordinal).Count() != 1)
        {
            throw new BridgeUnavailableException(
                $"FFmpeg option is missing or ambiguous: {string.Join('/', names)}");
        }
        return matches[0];
    }

    private static IReadOnlyList<string> TokenizeArguments(string arguments)
    {
        var tokens = new List<string>();
        var current = new StringBuilder();
        char quote = '\0';
        for (var index = 0; index < arguments.Length; index++)
        {
            var character = arguments[index];
            if (quote != '\0')
            {
                if (character == quote)
                {
                    quote = '\0';
                }
                else if (character == '\\' && index + 1 < arguments.Length
                         && arguments[index + 1] == quote)
                {
                    current.Append(arguments[++index]);
                }
                else
                {
                    current.Append(character);
                }
                continue;
            }
            if (character is '\'' or '"')
            {
                quote = character;
            }
            else if (char.IsWhiteSpace(character))
            {
                if (current.Length > 0)
                {
                    tokens.Add(current.ToString());
                    current.Clear();
                }
            }
            else
            {
                current.Append(character);
            }
        }
        if (quote != '\0')
        {
            throw new BridgeUnavailableException("FFmpeg arguments contain an unterminated quote");
        }
        if (current.Length > 0)
        {
            tokens.Add(current.ToString());
        }
        return tokens;
    }

    private static string NormalizeH264Encoder(string value)
    {
        var normalized = value.ToLowerInvariant();
        if (normalized is "h264" or "libx264"
            || normalized.StartsWith("h264_", StringComparison.Ordinal))
        {
            return "h264";
        }
        throw new BridgeUnavailableException($"Unsupported FFmpeg video encoder: {value}");
    }

    private static string NormalizeAacEncoder(string value)
    {
        if (string.Equals(value, "aac", StringComparison.OrdinalIgnoreCase))
        {
            // YMM4 4.55.1.1's active native FFmpeg AAC encoder defaults to AAC-LC.
            // The exact FFmpeg binary is leased and the final esds AudioSpecificConfig
            // is independently required to report audioObjectType 2.
            return "aac_lc";
        }
        throw new BridgeUnavailableException($"Unsupported FFmpeg audio encoder: {value}");
    }
}
