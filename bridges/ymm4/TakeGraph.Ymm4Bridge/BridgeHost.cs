using System.Net;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace TakeGraph.Ymm4Bridge;

internal static class BridgeJson
{
    internal static JsonSerializerOptions Options { get; } = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        PropertyNameCaseInsensitive = true,
        WriteIndented = true,
    };

    internal static JsonSerializerOptions RequestOptions { get; } = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        PropertyNameCaseInsensitive = false,
        RespectRequiredConstructorParameters = true,
        UnmappedMemberHandling = JsonUnmappedMemberHandling.Disallow,
    };
}

internal sealed class BridgeHost : IDisposable
{
    internal static string BaseUrl => $"http://127.0.0.1:{BridgeContract.Port}/";

    private readonly HttpListener listener = new();
    private readonly CancellationTokenSource cancellation = new();
    private readonly Ymm4Facade facade = new();
    private readonly BridgeCredentials credentials;
    private Task? listenTask;

    internal BridgeHost()
    {
        credentials = BridgeCredentials.LoadOrCreate();
    }

    internal bool IsRunning => listener.IsListening;

    internal string Status => IsRunning
        ? $"TakeGraph YMM4 Bridge: {BaseUrl}"
        : "TakeGraph YMM4 Bridge: stopped";

    internal void Start()
    {
        if (listener.IsListening)
        {
            return;
        }
        ApplyRequestDigest.ValidateCrossRuntimeGolden();
        RemarkCodec.ValidateRoundTrip();
        listener.Prefixes.Add(BaseUrl);
        listener.Start();
        facade.BeginStartupRecovery();
        listenTask = Task.Run(() => ListenAsync(cancellation.Token));
    }

    public void Dispose()
    {
        cancellation.Cancel();
        if (listener.IsListening)
        {
            listener.Stop();
        }
        listener.Close();
        cancellation.Dispose();
    }

    private async Task ListenAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested && listener.IsListening)
        {
            try
            {
                var context = await listener.GetContextAsync().WaitAsync(cancellationToken);
                _ = Task.Run(() => HandleAsync(context), cancellationToken);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (HttpListenerException) when (cancellationToken.IsCancellationRequested)
            {
                return;
            }
        }
    }

    private async Task HandleAsync(HttpListenerContext context)
    {
        try
        {
            if (!CryptographicEquals(context.Request.Headers[BridgeContract.TokenHeader], credentials.Token))
            {
                await WriteJsonAsync(context.Response, 401, new ErrorDto("Unauthorized"));
                return;
            }

            var path = context.Request.Url?.AbsolutePath.TrimEnd('/') ?? string.Empty;
            var method = context.Request.HttpMethod;
            object response;
            if (method == "GET" && path == "/v1/health")
            {
                response = Health();
            }
            else if (method == "GET" && path == "/v1/capabilities")
            {
                response = facade.Capabilities();
            }
            else if (method == "GET" && path == "/v2/descriptors")
            {
                response = facade.Descriptors();
            }
            else if (method == "GET" && path == "/v2/recovery")
            {
                response = facade.RecoveryStatus();
            }
            else if (method == "GET" && path == "/v2/project/checkpoint-profile")
            {
                response = facade.CheckpointProfile();
            }
            else if (method == "POST" && path == "/v2/project/checkpoints")
            {
                response = await facade.CreateCheckpointAsync(
                    await ReadJsonAsync<CheckpointRequestDto>(context.Request));
            }
            else if (method == "GET"
                && path.StartsWith("/v2/project/checkpoints/", StringComparison.Ordinal))
            {
                var idText = path["/v2/project/checkpoints/".Length..];
                if (!Guid.TryParse(idText, out var checkpointId))
                {
                    throw new BridgeValidationException("Invalid checkpoint operation ID");
                }
                response = facade.GetCheckpoint(checkpointId);
            }
            else if (method == "GET" && path == "/v2/render/profiles")
            {
                response = facade.RenderProfiles();
            }
            else if (method == "POST" && path == "/v2/render/tasks")
            {
                response = await facade.StartRenderAsync(
                    await ReadJsonAsync<RenderRequestDto>(context.Request));
            }
            else if (method == "POST"
                && path.StartsWith("/v2/render/tasks/", StringComparison.Ordinal)
                && path.EndsWith("/cancel", StringComparison.Ordinal))
            {
                var idText = path[
                    "/v2/render/tasks/".Length..^"/cancel".Length];
                if (!Guid.TryParse(idText, out var cancelTaskId))
                {
                    throw new BridgeValidationException("Invalid render task ID");
                }
                var cancelRequest = await ReadJsonAsync<RenderCancelRequestDto>(context.Request);
                if (cancelRequest.TaskId != cancelTaskId)
                {
                    throw new BridgeValidationException(
                        "Render cancellation path and payload task IDs differ");
                }
                response = facade.CancelRender(cancelRequest);
            }
            else if (method == "GET"
                && path.StartsWith("/v2/render/tasks/", StringComparison.Ordinal))
            {
                var idText = path["/v2/render/tasks/".Length..];
                if (!Guid.TryParse(idText, out var renderTaskId))
                {
                    throw new BridgeValidationException("Invalid render task ID");
                }
                response = facade.GetRender(renderTaskId);
            }
            else if (method == "GET" && path == "/v1/project/snapshot")
            {
                response = facade.Snapshot();
            }
            else if (method == "GET" && path == "/v1/project/controls")
            {
                response = facade.Controls();
            }
            else if (method == "POST" && path == "/v1/project/save")
            {
                response = await facade.SaveProjectAsync();
            }
            else if (method == "POST" && path == "/v1/project/undo")
            {
                response = facade.Undo();
            }
            else if (method == "POST" && path == "/v1/project/redo")
            {
                response = facade.Redo();
            }
            else if (method == "POST" && path == "/v1/application/close")
            {
                response = facade.ScheduleApplicationClose();
            }
            else if (method == "POST" && path == "/v1/managed/plan")
            {
                response = facade.Plan(await ReadJsonAsync<PlanRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v1/managed/apply")
            {
                response = await facade.ApplyAsync(await ReadJsonAsync<ApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/target-plan/validate")
            {
                response = facade.ValidateTargetPlan(
                    await ReadJsonAsync<TargetPlanRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/target-plan/apply")
            {
                response = await facade.ApplyTargetPlanAsync(
                    await ReadJsonAsync<TargetPlanApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/target-plan/not-started")
            {
                response = await facade.SealTargetPlanNotStartedAsync(
                    await ReadJsonAsync<TargetPlanApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-voice/plan")
            {
                response = facade.PlanNativeVoice(
                    await ReadJsonAsync<NativeVoicePlanRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-voice/apply")
            {
                response = await facade.ApplyNativeVoiceAsync(
                    await ReadJsonAsync<NativeVoiceApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-voice/mutation/plan")
            {
                response = facade.PlanNativeVoiceMutations(
                    await ReadJsonAsync<NativeVoiceMutationPlanRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-voice/mutation/apply")
            {
                response = await facade.ApplyNativeVoiceMutationsAsync(
                    await ReadJsonAsync<NativeVoiceMutationApplyRequestDto>(context.Request));
            }
            else if (method == "POST"
                && path == "/v2/native-voice/mutation/not-started")
            {
                response = await facade.SealNativeVoiceMutationNotStartedAsync(
                    await ReadJsonAsync<NativeVoiceMutationApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-voice/artifacts")
            {
                response = await facade.ExportNativeVoiceArtifactsAsync(
                    await ReadJsonAsync<NativeVoiceArtifactRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-extension/plan")
            {
                response = facade.PlanNativeExtensions(
                    await ReadJsonAsync<NativeExtensionPlanRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-extension/apply")
            {
                response = await facade.ApplyNativeExtensionsAsync(
                    await ReadJsonAsync<NativeExtensionApplyRequestDto>(context.Request));
            }
            else if (method == "POST" && path == "/v2/native-extension/not-started")
            {
                response = await facade.SealNativeExtensionNotStartedAsync(
                    await ReadJsonAsync<NativeExtensionApplyRequestDto>(context.Request));
            }
            else if (method == "GET"
                && path.StartsWith("/v2/native-extension/operations/", StringComparison.Ordinal))
            {
                var idText = path["/v2/native-extension/operations/".Length..];
                if (!Guid.TryParse(idText, out var nativeExtensionOperationId))
                {
                    throw new BridgeValidationException(
                        "Invalid native-extension operation ID");
                }
                response = facade.GetNativeExtensionOperation(nativeExtensionOperationId);
            }
            else if (method == "POST" && path == "/v2/reconciliation/detach")
            {
                response = await facade.DetachManagedMetadataAsync(
                    await ReadJsonAsync<MetadataDetachRequestDto>(context.Request));
            }
            else if (method == "POST"
                && path == "/v2/reconciliation/detach/not-started")
            {
                response = await facade.SealMetadataDetachNotStartedAsync(
                    await ReadJsonAsync<MetadataDetachRequestDto>(context.Request));
            }
            else if (method == "GET"
                && path.StartsWith("/v2/reconciliation/detach/", StringComparison.Ordinal))
            {
                var idText = path["/v2/reconciliation/detach/".Length..];
                if (!Guid.TryParse(idText, out var detachOperationId))
                {
                    throw new BridgeValidationException("Invalid metadata detach operation ID");
                }
                response = facade.GetMetadataDetach(detachOperationId);
            }
            else if (method == "POST" && path == "/v2/scene/capture")
            {
                response = await facade.CaptureSceneAsync(
                    await ReadJsonAsync<SceneCaptureRequestDto>(context.Request));
            }
            else if (method == "GET" && path.StartsWith("/v1/operations/", StringComparison.Ordinal))
            {
                var idText = path["/v1/operations/".Length..];
                if (!Guid.TryParse(idText, out var operationId))
                {
                    throw new BridgeValidationException("Invalid operation ID");
                }
                response = facade.GetOperation(operationId);
            }
            else
            {
                await WriteJsonAsync(context.Response, 404, new ErrorDto("Not found"));
                return;
            }
            await WriteJsonAsync(context.Response, 200, response);
        }
        catch (Exception error)
        {
            var mapped = MapError(error);
            await WriteJsonAsync(context.Response, mapped.StatusCode, mapped.Error);
        }
    }

    private static HealthDto Health()
    {
        var pluginVersion = typeof(BridgeHost).Assembly
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
            ?? typeof(BridgeHost).Assembly.GetName().Version?.ToString()
            ?? "unknown";
        var ymm4Version = Assembly.GetEntryAssembly()?.GetName().Version?.ToString() ?? "unknown";
        return new HealthDto(
            "running",
            BridgeContract.ProtocolVersion,
            pluginVersion,
            ymm4Version);
    }

    private static async Task<T> ReadJsonAsync<T>(HttpListenerRequest request)
    {
        using var reader = new StreamReader(request.InputStream, request.ContentEncoding ?? Encoding.UTF8);
        var body = await reader.ReadToEndAsync();
        return DeserializeRequest<T>(body);
    }

    internal static T DeserializeRequest<T>(string body)
    {
        try
        {
            using var document = JsonDocument.Parse(body);
            RejectDuplicateProperties(
                document.RootElement,
                "$",
                new HashSet<string>(StringComparer.Ordinal));
            return JsonSerializer.Deserialize<T>(
                    document.RootElement.GetRawText(),
                    BridgeJson.RequestOptions)
                ?? throw new BridgeValidationException("Request body is empty or invalid");
        }
        catch (BridgeValidationException)
        {
            throw;
        }
        catch (Exception error) when (error is JsonException or NotSupportedException)
        {
            throw new BridgeValidationException(
                $"Invalid request JSON: {error.GetBaseException().Message}");
        }
    }

    internal static (int StatusCode, ErrorDto Error) MapError(Exception error)
    {
        return error switch
        {
            BridgeConflictException conflict =>
                (409, new ErrorDto(conflict.Message, conflict.ActualFingerprint)),
            BridgeValidationException validation =>
                (400, new ErrorDto(validation.Message)),
            BridgeNotFoundException notFound =>
                (404, new ErrorDto(notFound.Message)),
            BridgeUnavailableException unavailable =>
                (503, new ErrorDto(unavailable.Message)),
            _ => (500, new ErrorDto(error.GetBaseException().Message)),
        };
    }

    internal static void RejectDuplicateProperties(
        JsonElement element,
        string path,
        HashSet<string> names)
    {
        switch (element.ValueKind)
        {
            case JsonValueKind.Object:
                names.Clear();
                foreach (var property in element.EnumerateObject())
                {
                    if (!names.Add(property.Name))
                    {
                        throw new BridgeValidationException(
                            $"Duplicate JSON property '{property.Name}' at {path}");
                    }
                    RejectDuplicateProperties(
                        property.Value,
                        $"{path}.{property.Name}",
                        new HashSet<string>(StringComparer.Ordinal));
                }
                break;
            case JsonValueKind.Array:
                var index = 0;
                foreach (var item in element.EnumerateArray())
                {
                    RejectDuplicateProperties(
                        item,
                        $"{path}[{index}]",
                        new HashSet<string>(StringComparer.Ordinal));
                    index += 1;
                }
                break;
        }
    }

    private static async Task WriteJsonAsync(HttpListenerResponse response, int status, object value)
    {
        if (response.OutputStream is null)
        {
            return;
        }
        response.StatusCode = status;
        response.ContentType = "application/json; charset=utf-8";
        response.Headers["Cache-Control"] = "no-store";
        var bytes = JsonSerializer.SerializeToUtf8Bytes(value, BridgeJson.Options);
        response.ContentLength64 = bytes.Length;
        await response.OutputStream.WriteAsync(bytes);
        response.Close();
    }

    private static bool CryptographicEquals(string? provided, string expected)
    {
        if (provided is null)
        {
            return false;
        }
        var providedBytes = Encoding.UTF8.GetBytes(provided);
        var expectedBytes = Encoding.UTF8.GetBytes(expected);
        return providedBytes.Length == expectedBytes.Length
            && System.Security.Cryptography.CryptographicOperations.FixedTimeEquals(
                providedBytes,
                expectedBytes);
    }
}
