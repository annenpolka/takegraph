namespace YukkuriMovieMaker.Settings
{
    internal sealed class ItemTemplate
    {
        internal string Name { get; init; } = string.Empty;

        internal IReadOnlyList<string> Path { get; init; } = [];

        internal string Group { get; init; } = string.Empty;

        internal string SceneId { get; init; } = string.Empty;

        internal IReadOnlyList<object> Items { get; init; } = [];

        internal int Width { get; init; }

        internal int Height { get; init; }

        internal int FPS { get; init; }

        internal int Hz { get; init; }
    }
}

internal sealed class TemplateMenuNode
{
    internal object? Template { get; set; }

    internal List<object> Items { get; } = [];
}

internal sealed class StrictRequestFixture
{
    public StrictRequestFixture(int protocolVersion, string projectId)
    {
        ProtocolVersion = protocolVersion;
        ProjectId = projectId;
    }

    public int ProtocolVersion { get; }

    public string ProjectId { get; }
}

internal sealed class StrictNestedRequestFixture
{
    public StrictNestedRequestFixture(int protocolVersion, System.Text.Json.JsonElement payload)
    {
        ProtocolVersion = protocolVersion;
        Payload = payload;
    }

    public int ProtocolVersion { get; }

    public System.Text.Json.JsonElement Payload { get; }
}

internal sealed class RequiredItemFixture
{
    internal int Frame { get; init; }

    internal int Layer { get; init; }

    internal int Length { get; init; }

    internal bool IsLocked { get; init; }
}

internal sealed class ThrowingRequiredItemFixture
{
    internal int Frame => throw new InvalidOperationException("frame getter failed");
}

internal sealed class InvalidRequiredItemFixture
{
    internal string Frame => "1.5";
}

internal sealed class ExactPreviewFrameFixture
{
    internal int CurrentFrame { get; init; }

    internal double CurrentPositionRate { get; init; }

    internal TimeSpan StartPosition { get; init; }
}

internal sealed class EstimatedPreviewFrameFixture
{
    internal double CurrentPositionRate { get; init; }

    internal TimeSpan StartPosition { get; init; }
}

internal sealed class ThrowingPreservationItemFixture
{
    public string Dangerous
    {
        get => throw new InvalidOperationException("preserved getter failed");
        set { }
    }

    public object KeyFrames { get; set; } = new();
}
