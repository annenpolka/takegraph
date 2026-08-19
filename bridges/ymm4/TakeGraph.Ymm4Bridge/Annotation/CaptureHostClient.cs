using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json;

namespace TakeGraph.Ymm4Bridge;

internal sealed class CaptureHostException(string message, HttpStatusCode? status = null)
    : Exception(message)
{
    internal HttpStatusCode? Status { get; } = status;
}

/// Thin loopback client of `takegraph annotation listen`.
internal sealed class CaptureHostClient : IDisposable
{
    internal const string TokenHeader = "x-takegraph-token";

    private readonly HttpClient http;
    private readonly string token;

    internal CaptureHostClient(string endpoint, string token, HttpMessageHandler? handler = null)
    {
        if (string.IsNullOrWhiteSpace(token))
        {
            throw new CaptureHostException("capture-host token must not be empty");
        }
        if (!Uri.TryCreate(endpoint, UriKind.Absolute, out var uri) || !IsLoopback(uri))
        {
            throw new CaptureHostException("capture-host endpoint must be loopback");
        }
        this.token = token;
        http = handler is null
            ? new HttpClient { BaseAddress = Normalize(uri), Timeout = TimeSpan.FromSeconds(3) }
            : new HttpClient(handler, disposeHandler: false)
            {
                BaseAddress = Normalize(uri),
                Timeout = TimeSpan.FromSeconds(3),
            };
        http.DefaultRequestHeaders.TryAddWithoutValidation(TokenHeader, token);
        http.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/json"));
    }

    internal string Token => token;

    internal static string DefaultCredentialsPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "TakeGraph",
        "capture-host.json");

    internal static CaptureHostClient? TryLoad(string? credentialsPath = null)
    {
        var path = credentialsPath ?? DefaultCredentialsPath;
        if (!File.Exists(path))
        {
            return null;
        }
        try
        {
            var credentials = JsonSerializer.Deserialize<CaptureHostCredentialsFile>(
                File.ReadAllText(path),
                BridgeJson.Options);
            if (credentials is null || string.IsNullOrWhiteSpace(credentials.Token))
            {
                return null;
            }
            return new CaptureHostClient(credentials.Endpoint, credentials.Token);
        }
        catch (Exception)
        {
            return null;
        }
    }

    internal Task<CaptureHostHealthDto> HealthAsync(CancellationToken cancellation = default) =>
        GetAsync<CaptureHostHealthDto>("/v1/health", cancellation);

    internal Task<CaptureHostStatusDto> StatusAsync(CancellationToken cancellation = default) =>
        GetAsync<CaptureHostStatusDto>("/v1/status", cancellation);

    internal Task<CaptureDevicesDto> DevicesAsync(CancellationToken cancellation = default) =>
        GetAsync<CaptureDevicesDto>("/v1/devices", cancellation);

    internal Task<CaptureAnnotationsDto> AnnotationsAsync(
        int limit = 20,
        string? projectId = null,
        CancellationToken cancellation = default)
    {
        var path = $"/v1/annotations?limit={limit}";
        if (!string.IsNullOrWhiteSpace(projectId))
        {
            path += $"&projectId={Uri.EscapeDataString(projectId)}";
        }
        return GetAsync<CaptureAnnotationsDto>(path, cancellation);
    }

    internal Task<CaptureStartDto> StartAsync(CancellationToken cancellation = default) =>
        PostAsync<CaptureStartDto>("/v1/capture/start", null, cancellation);

    internal Task<CaptureAnnotationDto> StopAsync(CancellationToken cancellation = default) =>
        PostAsync<CaptureAnnotationDto>("/v1/capture/stop", null, cancellation);

    internal Task CancelAsync(CancellationToken cancellation = default) =>
        PostEmptyAsync("/v1/capture/cancel", cancellation);

    internal Task DismissAsync(Guid annotationId, CancellationToken cancellation = default) =>
        PostEmptyAsync($"/v1/annotations/{annotationId:D}/dismiss", cancellation);

    internal Task SetDeviceAsync(string? deviceId, CancellationToken cancellation = default) =>
        PostEmptyAsync("/v1/config/device", new CaptureDeviceUpdateDto(deviceId), cancellation);

    internal Task<CaptureHotkeysDto> HotkeysAsync(CancellationToken cancellation = default) =>
        GetAsync<CaptureHotkeysDto>("/v1/hotkeys", cancellation);

    internal Task<CaptureHotkeysDto> SetHotkeyAsync(
        string hotkey,
        CancellationToken cancellation = default) =>
        PostAsync<CaptureHotkeysDto>(
            "/v1/config/hotkey",
            new CaptureHotkeyUpdateDto(hotkey),
            cancellation);

    public void Dispose() => http.Dispose();

    private async Task<T> GetAsync<T>(string path, CancellationToken cancellation)
    {
        using var response = await http.GetAsync(path, cancellation).ConfigureAwait(false);
        return await ReadAsync<T>(response, cancellation).ConfigureAwait(false);
    }

    private async Task<T> PostAsync<T>(string path, object? body, CancellationToken cancellation)
    {
        using var response = await SendPostAsync(path, body, cancellation).ConfigureAwait(false);
        return await ReadAsync<T>(response, cancellation).ConfigureAwait(false);
    }

    private async Task PostEmptyAsync(string path, CancellationToken cancellation) =>
        await PostEmptyAsync(path, null, cancellation).ConfigureAwait(false);

    private async Task PostEmptyAsync(string path, object? body, CancellationToken cancellation)
    {
        using var response = await SendPostAsync(path, body, cancellation).ConfigureAwait(false);
        await EnsureSuccessAsync(response, cancellation).ConfigureAwait(false);
    }

    private Task<HttpResponseMessage> SendPostAsync(
        string path,
        object? body,
        CancellationToken cancellation)
    {
        return body is null
            ? http.PostAsync(path, null, cancellation)
            : http.PostAsJsonAsync(path, body, BridgeJson.Options, cancellation);
    }

    private static async Task<T> ReadAsync<T>(
        HttpResponseMessage response,
        CancellationToken cancellation)
    {
        await EnsureSuccessAsync(response, cancellation).ConfigureAwait(false);
        var value = await response.Content
            .ReadFromJsonAsync<T>(BridgeJson.Options, cancellation)
            .ConfigureAwait(false);
        return value ?? throw new CaptureHostException("capture host returned an empty JSON body");
    }

    private static async Task EnsureSuccessAsync(
        HttpResponseMessage response,
        CancellationToken cancellation)
    {
        if (response.IsSuccessStatusCode)
        {
            return;
        }
        var detail = await response.Content.ReadAsStringAsync(cancellation).ConfigureAwait(false);
        try
        {
            var error = JsonSerializer.Deserialize<CaptureHostErrorDto>(detail, BridgeJson.Options);
            if (!string.IsNullOrWhiteSpace(error?.Error))
            {
                throw new CaptureHostException(error.Error, response.StatusCode);
            }
        }
        catch (CaptureHostException)
        {
            throw;
        }
        catch (JsonException)
        {
            // Fall through to the raw body.
        }
        throw new CaptureHostException(
            string.IsNullOrWhiteSpace(detail) ? response.ReasonPhrase ?? "capture host request failed" : detail,
            response.StatusCode);
    }

    internal static bool IsLoopback(Uri endpoint)
    {
        if (endpoint.Scheme != Uri.UriSchemeHttp && endpoint.Scheme != Uri.UriSchemeHttps)
        {
            return false;
        }
        if (string.Equals(endpoint.Host, "localhost", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }
        return IPAddress.TryParse(endpoint.Host, out var address) && IPAddress.IsLoopback(address);
    }

    private static Uri Normalize(Uri endpoint)
    {
        var builder = new UriBuilder(endpoint);
        if (!builder.Path.EndsWith('/'))
        {
            builder.Path += "/";
        }
        return builder.Uri;
    }
}
