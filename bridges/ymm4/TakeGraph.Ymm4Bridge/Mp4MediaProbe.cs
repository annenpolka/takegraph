using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;

namespace TakeGraph.Ymm4Bridge;

internal static class Mp4MediaProbe
{
    internal static RenderedMediaReceiptDto Probe(string path)
    {
        var fullPath = Path.GetFullPath(path);
        using var namespaceLease = RenderPathNamespaceLease.Open([fullPath], []);
        using var stream = new FileStream(fullPath, FileMode.Open, FileAccess.Read, FileShare.Read);
        RenderPathNamespaceLease.VerifyFileHandlePath(stream, fullPath);
        if (stream.Length < 12)
        {
            throw new InvalidDataException("Rendered output is too short to be an MP4 file");
        }
        Span<byte> header = stackalloc byte[12];
        stream.ReadExactly(header);
        if (!header[4..8].SequenceEqual("ftyp"u8))
        {
            throw new InvalidDataException("Rendered output is not an ISO BMFF/MP4 file");
        }

        var result = new ProbeAccumulator();
        ScanBoxes(stream, 0, checked((ulong)stream.Length), 0, result, null);
        if (result.DurationMillis is null
            || result.Width is null
            || result.Height is null
            || result.VideoStreams != 1
            || result.AudioStreams > 1
            || result.VideoCodec is null
            || result.PixelFormat is null
            || result.FpsNumerator is null
            || result.FpsDenominator is null
            || (result.AudioStreams > 0
                && (result.AudioCodec is null || result.AudioSampleRate is null)))
        {
            throw new InvalidDataException("Rendered MP4 is missing duration, dimensions, or a video stream");
        }

        stream.Position = 0;
        var sha256 = Convert.ToHexStringLower(SHA256.HashData(stream));
        var byteLength = checked((ulong)stream.Length);
        var digest = ComputeDigest(
            sha256,
            byteLength,
            result.DurationMillis.Value,
            result.Width.Value,
            result.Height.Value,
            result.VideoStreams,
            result.AudioStreams,
            result.FpsNumerator.Value,
            result.FpsDenominator.Value,
            result.VideoCodec,
            result.AudioCodec,
            result.AudioSampleRate,
            result.PixelFormat);
        namespaceLease.Verify();
        return new RenderedMediaReceiptDto(
            fullPath,
            sha256,
            byteLength,
            "mp4",
            result.DurationMillis.Value,
            result.Width.Value,
            result.Height.Value,
            result.VideoStreams,
            result.AudioStreams,
            result.FpsNumerator.Value,
            result.FpsDenominator.Value,
            result.VideoCodec,
            result.AudioCodec,
            result.AudioSampleRate,
            result.PixelFormat,
            "takegraph-final-media-probe/mp4-v3",
            digest);
    }

    private static void ScanBoxes(
        Stream stream,
        ulong start,
        ulong end,
        byte depth,
        ProbeAccumulator result,
        TrackProbe? track)
    {
        if (depth > 7)
        {
            return;
        }
        var offset = start;
        Span<byte> header = stackalloc byte[16];
        while (offset <= end && end - offset >= 8)
        {
            stream.Position = checked((long)offset);
            stream.ReadExactly(header[..8]);
            var shortSize = BinaryPrimitives.ReadUInt32BigEndian(header[..4]);
            var kind = Encoding.ASCII.GetString(header[4..8]);
            ulong headerSize;
            ulong boxSize;
            if (shortSize == 1)
            {
                stream.ReadExactly(header[8..16]);
                headerSize = 16;
                boxSize = BinaryPrimitives.ReadUInt64BigEndian(header[8..16]);
            }
            else if (shortSize == 0)
            {
                headerSize = 8;
                boxSize = end - offset;
            }
            else
            {
                headerSize = 8;
                boxSize = shortSize;
            }
            if (boxSize < headerSize || boxSize > end - offset)
            {
                throw new InvalidDataException("Rendered MP4 contains a malformed box");
            }
            var dataStart = checked(offset + headerSize);
            var boxEnd = checked(offset + boxSize);
            switch (kind)
            {
                case "moov":
                case "mdia":
                case "minf":
                case "stbl":
                    ScanBoxes(
                        stream,
                        dataStart,
                        boxEnd,
                        checked((byte)(depth + 1)),
                        result,
                        track);
                    break;
                case "trak":
                    var childTrack = new TrackProbe();
                    ScanBoxes(
                        stream,
                        dataStart,
                        boxEnd,
                        checked((byte)(depth + 1)),
                        result,
                        childTrack);
                    CompleteTrack(childTrack, result);
                    break;
                case "mvhd":
                    ReadMovieHeader(stream, dataStart, boxEnd, result);
                    break;
                case "tkhd":
                    ReadTrackHeader(stream, dataStart, boxEnd, track);
                    break;
                case "hdlr":
                    ReadHandler(stream, dataStart, boxEnd, track);
                    break;
                case "mdhd":
                    ReadMediaHeader(stream, dataStart, boxEnd, track);
                    break;
                case "stts":
                    ReadTimeToSample(stream, dataStart, boxEnd, track);
                    break;
                case "stsd":
                    ReadSampleDescription(stream, dataStart, boxEnd, track);
                    break;
            }
            offset = boxEnd;
        }
    }

    private static void ReadMovieHeader(
        Stream stream,
        ulong start,
        ulong end,
        ProbeAccumulator result)
    {
        stream.Position = checked((long)start);
        Span<byte> data = stackalloc byte[32];
        var available = checked((int)Math.Min(end - start, (ulong)data.Length));
        stream.ReadExactly(data[..available]);
        uint timescale;
        ulong duration;
        if (data[0] == 0 && available >= 20)
        {
            timescale = BinaryPrimitives.ReadUInt32BigEndian(data[12..16]);
            duration = BinaryPrimitives.ReadUInt32BigEndian(data[16..20]);
        }
        else if (data[0] == 1 && available >= 32)
        {
            timescale = BinaryPrimitives.ReadUInt32BigEndian(data[20..24]);
            duration = BinaryPrimitives.ReadUInt64BigEndian(data[24..32]);
        }
        else
        {
            throw new InvalidDataException("Rendered MP4 contains an unsupported movie header");
        }
        if (timescale == 0)
        {
            throw new InvalidDataException("Rendered MP4 has a zero timescale");
        }
        result.DurationMillis = SaturatingMultiply(duration, 1_000) / timescale;
    }

    private static void ReadTrackHeader(
        Stream stream,
        ulong start,
        ulong end,
        TrackProbe? track)
    {
        if (track is null)
        {
            throw new InvalidDataException("Rendered MP4 has a track header outside a track");
        }
        stream.Position = checked((long)start);
        Span<byte> data = stackalloc byte[104];
        var available = checked((int)Math.Min(end - start, (ulong)data.Length));
        stream.ReadExactly(data[..available]);
        var offset = data[0] switch
        {
            0 when available >= 84 => 76,
            1 when available >= 96 => 88,
            _ => throw new InvalidDataException("Rendered MP4 contains an unsupported track header"),
        };
        var width = BinaryPrimitives.ReadUInt32BigEndian(data[offset..(offset + 4)]) >> 16;
        var height = BinaryPrimitives.ReadUInt32BigEndian(data[(offset + 4)..(offset + 8)]) >> 16;
        if (width > 0 && height > 0)
        {
            if (track.Width is not null || track.Height is not null)
            {
                throw new InvalidDataException("Rendered MP4 track has multiple dimensions");
            }
            track.Width = width;
            track.Height = height;
        }
    }

    private static void ReadHandler(
        Stream stream,
        ulong start,
        ulong end,
        TrackProbe? track)
    {
        if (track is null)
        {
            throw new InvalidDataException("Rendered MP4 has a handler outside a track");
        }
        if (end - start < 12)
        {
            throw new InvalidDataException("Rendered MP4 contains a malformed handler box");
        }
        stream.Position = checked((long)(start + 8));
        Span<byte> handler = stackalloc byte[4];
        stream.ReadExactly(handler);
        track.Handler = Encoding.ASCII.GetString(handler);
    }

    private static void ReadMediaHeader(
        Stream stream,
        ulong start,
        ulong end,
        TrackProbe? track)
    {
        if (track is null)
        {
            throw new InvalidDataException("Rendered MP4 has a media header outside a track");
        }
        stream.Position = checked((long)start);
        Span<byte> data = stackalloc byte[24];
        var available = checked((int)Math.Min(end - start, (ulong)data.Length));
        stream.ReadExactly(data[..available]);
        var timescale = data[0] switch
        {
            0 when available >= 16 => BinaryPrimitives.ReadUInt32BigEndian(data[12..16]),
            1 when available >= 24 => BinaryPrimitives.ReadUInt32BigEndian(data[20..24]),
            _ => throw new InvalidDataException("Rendered MP4 contains an unsupported media header"),
        };
        if (timescale == 0 || track.Timescale is not null)
        {
            throw new InvalidDataException("Rendered MP4 has an invalid media timescale");
        }
        track.Timescale = timescale;
    }

    private static void ReadTimeToSample(
        Stream stream,
        ulong start,
        ulong end,
        TrackProbe? track)
    {
        if (track is null || end - start < 8 || track.TimeToSampleSeen)
        {
            throw new InvalidDataException("Rendered MP4 contains an invalid time-to-sample table");
        }
        track.TimeToSampleSeen = true;
        stream.Position = checked((long)start);
        Span<byte> header = stackalloc byte[8];
        stream.ReadExactly(header);
        var entryCount = BinaryPrimitives.ReadUInt32BigEndian(header[4..8]);
        if (entryCount == 0 || entryCount > 1_000_000 || end - start != 8ul + entryCount * 8ul)
        {
            throw new InvalidDataException("Rendered MP4 contains an invalid time-to-sample table");
        }
        Span<byte> entry = stackalloc byte[8];
        uint? singleDelta = null;
        for (uint index = 0; index < entryCount; index++)
        {
            stream.ReadExactly(entry);
            var sampleCount = BinaryPrimitives.ReadUInt32BigEndian(entry[..4]);
            var sampleDelta = BinaryPrimitives.ReadUInt32BigEndian(entry[4..8]);
            if (sampleCount == 0 || sampleDelta == 0)
            {
                throw new InvalidDataException("Rendered MP4 contains an invalid time-to-sample entry");
            }
            if (entryCount == 1)
            {
                singleDelta = sampleDelta;
            }
        }
        track.SampleDelta = singleDelta;
    }

    private static void ReadSampleDescription(
        Stream stream,
        ulong start,
        ulong end,
        TrackProbe? track)
    {
        if (track is null || end - start < 8)
        {
            throw new InvalidDataException("Rendered MP4 contains a malformed sample description");
        }
        stream.Position = checked((long)(start + 4));
        Span<byte> countBytes = stackalloc byte[4];
        stream.ReadExactly(countBytes);
        var count = BinaryPrimitives.ReadUInt32BigEndian(countBytes);
        if (count == 0 || count > 64)
        {
            throw new InvalidDataException("Rendered MP4 has an unsupported sample-entry count");
        }
        var offset = start + 8;
        Span<byte> header = stackalloc byte[8];
        for (uint index = 0; index < count; index++)
        {
            if (offset > end || end - offset < 8)
            {
                throw new InvalidDataException("Rendered MP4 contains a truncated sample entry");
            }
            stream.Position = checked((long)offset);
            stream.ReadExactly(header);
            var size = BinaryPrimitives.ReadUInt32BigEndian(header[..4]);
            if (size < 8 || size > end - offset)
            {
                throw new InvalidDataException("Rendered MP4 contains a malformed sample entry");
            }
            var type = Encoding.ASCII.GetString(header[4..8]);
            var entryEnd = offset + size;
            var details = type switch
            {
                "avc1" or "avc3" => ReadAvcSampleEntry(stream, offset, entryEnd, type),
                "mp4a" => ReadMp4aSampleEntry(stream, offset, entryEnd),
                _ => new SampleEntryProbe(type, null, null, null, null),
            };
            track.SampleEntries.Add(details);
            offset += size;
        }
    }

    private static SampleEntryProbe ReadAvcSampleEntry(
        Stream stream,
        ulong start,
        ulong end,
        string type)
    {
        const ulong fixedLength = 8 + 78;
        if (end - start < fixedLength)
        {
            throw new InvalidDataException("Rendered MP4 contains a truncated AVC sample entry");
        }
        var avcConfig = ReadUniqueChildBox(stream, start + fixedLength, end, "avcC");
        var pixelFormat = ParseAvcPixelFormat(avcConfig);
        return new SampleEntryProbe(type, "h264", null, null, pixelFormat);
    }

    private static SampleEntryProbe ReadMp4aSampleEntry(Stream stream, ulong start, ulong end)
    {
        const ulong fixedLength = 8 + 28;
        if (end - start < fixedLength)
        {
            throw new InvalidDataException("Rendered MP4 contains a truncated mp4a sample entry");
        }
        Span<byte> fixedData = stackalloc byte[28];
        stream.Position = checked((long)(start + 8));
        stream.ReadExactly(fixedData);
        var fixedSampleRate = BinaryPrimitives.ReadUInt32BigEndian(fixedData[24..28]);
        if ((fixedSampleRate & 0xffff) != 0 || fixedSampleRate >> 16 == 0)
        {
            throw new InvalidDataException(
                "Rendered MP4 mp4a sample rate is not an integral 16.16 value");
        }
        var esds = ReadUniqueChildBox(stream, start + fixedLength, end, "esds");
        var (audioCodec, configSampleRate) = ParseAudioSpecificConfig(esds);
        var sampleRate = fixedSampleRate >> 16;
        if (sampleRate != configSampleRate)
        {
            throw new InvalidDataException(
                "Rendered MP4 mp4a and AudioSpecificConfig sample rates differ");
        }
        return new SampleEntryProbe("mp4a", null, audioCodec, sampleRate, null);
    }

    private static byte[] ReadUniqueChildBox(
        Stream stream,
        ulong start,
        ulong end,
        string requiredKind)
    {
        byte[]? payload = null;
        var offset = start;
        Span<byte> header = stackalloc byte[16];
        while (offset <= end && end - offset >= 8)
        {
            stream.Position = checked((long)offset);
            stream.ReadExactly(header[..8]);
            var shortSize = BinaryPrimitives.ReadUInt32BigEndian(header[..4]);
            ulong headerSize = 8;
            ulong boxSize;
            if (shortSize == 1)
            {
                stream.ReadExactly(header[8..16]);
                headerSize = 16;
                boxSize = BinaryPrimitives.ReadUInt64BigEndian(header[8..16]);
            }
            else
            {
                boxSize = shortSize == 0 ? end - offset : shortSize;
            }
            if (boxSize < headerSize || boxSize > end - offset)
            {
                throw new InvalidDataException("Rendered MP4 contains a malformed sample-entry child box");
            }
            var kind = Encoding.ASCII.GetString(header[4..8]);
            if (kind == requiredKind)
            {
                if (payload is not null || boxSize - headerSize > 1024 * 1024)
                {
                    throw new InvalidDataException(
                        $"Rendered MP4 contains an ambiguous {requiredKind} box");
                }
                payload = new byte[checked((int)(boxSize - headerSize))];
                stream.ReadExactly(payload);
            }
            offset += boxSize;
        }
        return payload
            ?? throw new InvalidDataException($"Rendered MP4 is missing required {requiredKind} evidence");
    }

    private static string ParseAvcPixelFormat(ReadOnlySpan<byte> config)
    {
        if (config.Length < 8 || config[0] != 1)
        {
            throw new InvalidDataException("Rendered MP4 has a malformed AVC configuration");
        }
        var count = config[5] & 0x1f;
        if (count == 0)
        {
            throw new InvalidDataException("Rendered MP4 AVC configuration has no SPS");
        }
        var offset = 6;
        string? pixelFormat = null;
        for (var index = 0; index < count; index++)
        {
            if (offset + 2 > config.Length)
            {
                throw new InvalidDataException("Rendered MP4 has a truncated SPS length");
            }
            var length = BinaryPrimitives.ReadUInt16BigEndian(config[offset..(offset + 2)]);
            offset += 2;
            if (length < 4 || offset + length > config.Length)
            {
                throw new InvalidDataException("Rendered MP4 has a truncated SPS");
            }
            var measured = ParseSpsPixelFormat(config.Slice(offset, length));
            pixelFormat = MergeCodec(pixelFormat, measured, "pixel format");
            offset += length;
        }
        return pixelFormat
            ?? throw new InvalidDataException("Rendered MP4 AVC SPS has no pixel-format evidence");
    }

    private static string ParseSpsPixelFormat(ReadOnlySpan<byte> sps)
    {
        if ((sps[0] & 0x1f) != 7)
        {
            throw new InvalidDataException("Rendered MP4 AVC configuration contains a non-SPS NAL");
        }
        var rbsp = new List<byte>(sps.Length);
        var zeroes = 0;
        foreach (var value in sps[1..])
        {
            if (zeroes >= 2 && value == 3)
            {
                zeroes = 0;
                continue;
            }
            rbsp.Add(value);
            zeroes = value == 0 ? zeroes + 1 : 0;
        }
        var bits = new AvcBitReader(rbsp.ToArray());
        var profile = bits.ReadBits(8);
        _ = bits.ReadBits(8);
        _ = bits.ReadBits(8);
        _ = bits.ReadUnsignedExpGolomb();
        uint chromaFormat = 1;
        uint bitDepthLumaMinus8 = 0;
        uint bitDepthChromaMinus8 = 0;
        var separateColourPlane = false;
        if (profile is 100 or 110 or 122 or 244 or 44 or 83 or 86 or 118 or 128 or 138 or 139 or 134 or 135)
        {
            chromaFormat = bits.ReadUnsignedExpGolomb();
            if (chromaFormat == 3)
            {
                separateColourPlane = bits.ReadBits(1) != 0;
            }
            bitDepthLumaMinus8 = bits.ReadUnsignedExpGolomb();
            bitDepthChromaMinus8 = bits.ReadUnsignedExpGolomb();
        }
        if (chromaFormat != 1
            || separateColourPlane
            || bitDepthLumaMinus8 != 0
            || bitDepthChromaMinus8 != 0)
        {
            throw new InvalidDataException("Rendered MP4 AVC SPS is not 8-bit 4:2:0");
        }
        return "yuv420p";
    }

    private static (string Codec, uint SampleRate) ParseAudioSpecificConfig(ReadOnlySpan<byte> esds)
    {
        if (esds.Length < 8)
        {
            throw new InvalidDataException("Rendered MP4 has a truncated esds box");
        }
        var offset = 4;
        var es = ReadDescriptor(esds, ref offset, 0x03);
        var esOffset = 0;
        if (es.Length < 3)
        {
            throw new InvalidDataException("Rendered MP4 has a malformed ES descriptor");
        }
        var flags = es[2];
        esOffset = 3;
        if ((flags & 0x80) != 0)
        {
            esOffset += 2;
        }
        if ((flags & 0x40) != 0)
        {
            if (esOffset >= es.Length)
            {
                throw new InvalidDataException("Rendered MP4 has a malformed ES URL flag");
            }
            esOffset += 1 + es[esOffset];
        }
        if ((flags & 0x20) != 0)
        {
            esOffset += 2;
        }
        var decoder = ReadDescriptor(es, ref esOffset, 0x04);
        if (decoder.Length < 15 || decoder[0] != 0x40)
        {
            throw new InvalidDataException(
                "Rendered MP4 mp4a entry is not MPEG-4 Audio objectTypeIndication 0x40");
        }
        var decoderOffset = 13;
        var config = ReadDescriptor(decoder, ref decoderOffset, 0x05);
        if (config.IsEmpty)
        {
            throw new InvalidDataException("Rendered MP4 has no AudioSpecificConfig");
        }
        var bits = new AudioBitReader(config);
        var audioObjectType = bits.ReadBits(5);
        if (audioObjectType != 2)
        {
            throw new InvalidDataException(
                $"Rendered MP4 audioObjectType is not AAC-LC: {audioObjectType}");
        }
        var frequencyIndex = bits.ReadBits(4);
        uint sampleRate = frequencyIndex switch
        {
            0 => 96_000,
            1 => 88_200,
            2 => 64_000,
            3 => 48_000,
            4 => 44_100,
            5 => 32_000,
            6 => 24_000,
            7 => 22_050,
            8 => 16_000,
            9 => 12_000,
            10 => 11_025,
            11 => 8_000,
            12 => 7_350,
            15 => bits.ReadBits(24),
            _ => throw new InvalidDataException(
                $"Rendered MP4 has a reserved AAC samplingFrequencyIndex: {frequencyIndex}"),
        };
        if (sampleRate == 0)
        {
            throw new InvalidDataException("Rendered MP4 AAC sample rate is zero");
        }
        return ("aac_lc", sampleRate);
    }

    private static ReadOnlySpan<byte> ReadDescriptor(
        ReadOnlySpan<byte> source,
        ref int offset,
        byte expectedTag)
    {
        if (offset >= source.Length || source[offset++] != expectedTag)
        {
            throw new InvalidDataException(
                $"Rendered MP4 is missing descriptor 0x{expectedTag:x2}");
        }
        var length = 0;
        var terminated = false;
        for (var index = 0; index < 4; index++)
        {
            if (offset >= source.Length)
            {
                throw new InvalidDataException("Rendered MP4 has a truncated descriptor length");
            }
            var value = source[offset++];
            length = checked((length << 7) | (value & 0x7f));
            if ((value & 0x80) == 0)
            {
                terminated = true;
                break;
            }
        }
        if (!terminated || length < 0 || offset + length > source.Length)
        {
            throw new InvalidDataException("Rendered MP4 has an invalid descriptor length");
        }
        var payload = source.Slice(offset, length);
        offset += length;
        return payload;
    }

    private static void CompleteTrack(TrackProbe track, ProbeAccumulator result)
    {
        if (track.Handler == "vide")
        {
            if (track.Timescale is null || track.SampleDelta is null)
            {
                throw new InvalidDataException("Rendered MP4 video track has no exact frame rate");
            }
            result.VideoStreams = result.VideoStreams == uint.MaxValue
                ? uint.MaxValue
                : result.VideoStreams + 1;
            if (track.Width is null || track.Height is null)
            {
                throw new InvalidDataException("Rendered MP4 video track has no dimensions");
            }
            result.Width = MergeNumber(result.Width, track.Width.Value, "video width");
            result.Height = MergeNumber(result.Height, track.Height.Value, "video height");
            var codecs = track.SampleEntries
                .Select(value => value.VideoCodec)
                .Where(value => value is not null)
                .Distinct(StringComparer.Ordinal)
                .ToArray();
            var pixelFormats = track.SampleEntries
                .Select(value => value.PixelFormat)
                .Where(value => value is not null)
                .Distinct(StringComparer.Ordinal)
                .ToArray();
            if (codecs is not [string videoCodec]
                || pixelFormats is not [string pixelFormat]
                || track.SampleEntries.Any(value => value.Type is not ("avc1" or "avc3")))
            {
                throw new InvalidDataException("Rendered MP4 video codec is not exactly H.264");
            }
            result.VideoCodec = MergeCodec(result.VideoCodec, videoCodec, "video");
            result.PixelFormat = MergeCodec(result.PixelFormat, pixelFormat, "pixel format");
            var divisor = GreatestCommonDivisor(track.Timescale.Value, track.SampleDelta.Value);
            var numerator = track.Timescale.Value / divisor;
            var denominator = track.SampleDelta.Value / divisor;
            result.FpsNumerator = MergeNumber(result.FpsNumerator, numerator, "frame-rate numerator");
            result.FpsDenominator = MergeNumber(result.FpsDenominator, denominator, "frame-rate denominator");
        }
        else if (track.Handler == "soun")
        {
            result.AudioStreams = result.AudioStreams == uint.MaxValue
                ? uint.MaxValue
                : result.AudioStreams + 1;
            var codecs = track.SampleEntries
                .Select(value => value.AudioCodec)
                .Where(value => value is not null)
                .Distinct(StringComparer.Ordinal)
                .ToArray();
            if (track.SampleEntries.Count != 1
                || track.SampleEntries[0].Type != "mp4a"
                || codecs is not [string audioCodec]
                || track.SampleEntries[0].AudioSampleRate is not uint audioSampleRate
                || track.Timescale != audioSampleRate)
            {
                throw new InvalidDataException("Rendered MP4 audio codec is not exactly AAC-LC");
            }
            result.AudioCodec = MergeCodec(result.AudioCodec, audioCodec, "audio");
            result.AudioSampleRate = MergeNumber(
                result.AudioSampleRate,
                audioSampleRate,
                "audio sample rate");
        }
    }

    private static string MergeCodec(string? existing, string value, string kind)
    {
        if (existing is not null && !string.Equals(existing, value, StringComparison.Ordinal))
        {
            throw new InvalidDataException($"Rendered MP4 has conflicting {kind} codecs");
        }
        return value;
    }

    private static uint MergeNumber(uint? existing, uint value, string kind)
    {
        if (existing is not null && existing.Value != value)
        {
            throw new InvalidDataException($"Rendered MP4 has conflicting {kind}");
        }
        return value;
    }

    private static uint GreatestCommonDivisor(uint left, uint right)
    {
        while (right != 0)
        {
            (left, right) = (right, left % right);
        }
        return left;
    }

    private static string ComputeDigest(
        string sha256,
        ulong byteLength,
        ulong durationMillis,
        uint width,
        uint height,
        uint videoStreams,
        uint audioStreams,
        uint fpsNumerator,
        uint fpsDenominator,
        string videoCodec,
        string? audioCodec,
        uint? audioSampleRate,
        string pixelFormat)
    {
        var canonical = new StringBuilder("takegraph-final-media-probe-v3\n");
        AppendString(canonical, "sha256", sha256);
        AppendNumber(canonical, "byteLength", byteLength);
        AppendString(canonical, "container", "mp4");
        AppendNumber(canonical, "durationMillis", durationMillis);
        AppendNumber(canonical, "width", width);
        AppendNumber(canonical, "height", height);
        AppendNumber(canonical, "videoStreams", videoStreams);
        AppendNumber(canonical, "audioStreams", audioStreams);
        AppendNumber(canonical, "fpsNumerator", fpsNumerator);
        AppendNumber(canonical, "fpsDenominator", fpsDenominator);
        AppendString(canonical, "videoCodec", videoCodec);
        AppendString(canonical, "audioCodec", audioCodec ?? string.Empty);
        AppendString(
            canonical,
            "audioSampleRate",
            audioSampleRate?.ToString(System.Globalization.CultureInfo.InvariantCulture)
                ?? string.Empty);
        AppendString(canonical, "pixelFormat", pixelFormat);
        AppendString(canonical, "probeProfile", "takegraph-final-media-probe/mp4-v3");
        return Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(canonical.ToString())));
    }

    private static void AppendString(StringBuilder value, string label, string text)
    {
        value.Append(label)
            .Append(':')
            .Append(Encoding.UTF8.GetByteCount(text))
            .Append(':')
            .Append(text)
            .Append('\n');
    }

    private static void AppendNumber<T>(StringBuilder value, string label, T number)
        where T : IFormattable
    {
        value.Append(label)
            .Append(':')
            .Append(number.ToString(null, System.Globalization.CultureInfo.InvariantCulture))
            .Append('\n');
    }

    private static ulong SaturatingMultiply(ulong left, ulong right)
    {
        return left > ulong.MaxValue / right ? ulong.MaxValue : left * right;
    }

    private sealed class ProbeAccumulator
    {
        internal ulong? DurationMillis { get; set; }
        internal uint? Width { get; set; }
        internal uint? Height { get; set; }
        internal uint VideoStreams { get; set; }
        internal uint AudioStreams { get; set; }
        internal string? VideoCodec { get; set; }
        internal string? AudioCodec { get; set; }
        internal uint? AudioSampleRate { get; set; }
        internal string? PixelFormat { get; set; }
        internal uint? FpsNumerator { get; set; }
        internal uint? FpsDenominator { get; set; }
    }

    private sealed class TrackProbe
    {
        internal string? Handler { get; set; }
        internal uint? Timescale { get; set; }
        internal uint? Width { get; set; }
        internal uint? Height { get; set; }
        internal uint? SampleDelta { get; set; }
        internal bool TimeToSampleSeen { get; set; }
        internal List<SampleEntryProbe> SampleEntries { get; } = [];
    }

    private sealed record SampleEntryProbe(
        string Type,
        string? VideoCodec,
        string? AudioCodec,
        uint? AudioSampleRate,
        string? PixelFormat);

    private ref struct AudioBitReader(ReadOnlySpan<byte> bytes)
    {
        private readonly ReadOnlySpan<byte> bytes = bytes;
        private int bitOffset;

        internal uint ReadBits(int count)
        {
            if (count is < 0 or > 32 || bitOffset + count > bytes.Length * 8)
            {
                throw new InvalidDataException("Rendered MP4 AudioSpecificConfig is truncated");
            }
            uint value = 0;
            for (var index = 0; index < count; index++)
            {
                value = (value << 1)
                    | (uint)((bytes[bitOffset / 8] >> (7 - (bitOffset % 8))) & 1);
                bitOffset++;
            }
            return value;
        }
    }

    private ref struct AvcBitReader(ReadOnlySpan<byte> bytes)
    {
        private readonly ReadOnlySpan<byte> bytes = bytes;
        private int bitOffset;

        internal uint ReadBits(int count)
        {
            if (count is < 0 or > 32 || bitOffset + count > bytes.Length * 8)
            {
                throw new InvalidDataException("Rendered MP4 AVC SPS is truncated");
            }
            uint value = 0;
            for (var index = 0; index < count; index++)
            {
                value = (value << 1)
                    | (uint)((bytes[bitOffset / 8] >> (7 - (bitOffset % 8))) & 1);
                bitOffset++;
            }
            return value;
        }

        internal uint ReadUnsignedExpGolomb()
        {
            var zeroes = 0;
            while (ReadBits(1) == 0)
            {
                zeroes++;
                if (zeroes > 31)
                {
                    throw new InvalidDataException("Rendered MP4 AVC SPS has an oversized code");
                }
            }
            return zeroes == 0
                ? 0
                : checked(((1u << zeroes) - 1) + ReadBits(zeroes));
        }
    }
}
