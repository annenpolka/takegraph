import {
  RESOURCE_MIME_TYPE,
  registerAppResource,
  registerAppTool,
} from "@modelcontextprotocol/ext-apps/server";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { z } from "zod";
import { ProjectSession, type ProjectState } from "./project-session.js";
import { Ymm4Workflow } from "./ymm4-workflow.js";

const resourceUri = "ui://takegraph/editor/v1.html";
const viewDirectory = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "studio-view",
  "dist",
);

export interface CreateServerOptions {
  session?: ProjectSession;
  viewHtml?: string;
  ymm4Workflow?: Ymm4Workflow;
  /** Trusted test/embedding override for the scene content-addressed root. */
  sceneArtifactRoot?: string;
}

function stateResult(state: ProjectState, message: string) {
  return {
    content: [{ type: "text" as const, text: message }],
    structuredContent: { state },
  };
}

function errorResult(error: unknown) {
  return {
    isError: true,
    content: [
      {
        type: "text" as const,
        text: error instanceof Error ? error.message : String(error),
      },
    ],
  };
}

const pngSignature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const maxSceneImageCount = 16;
const maxSceneImageBytes = 8 * 1024 * 1024;
const maxSceneImageTotalBytes = 32 * 1024 * 1024;
const maxSceneImagePixels = 33_177_600; // 7680 x 4320 (8K UHD)

interface SceneCaptureArtifact {
  artifactPath?: string;
  sha256?: string;
  mediaType?: string;
  width?: number;
  height?: number;
}

function sceneCaptures(result: unknown): SceneCaptureArtifact[] {
  if (result === null || typeof result !== "object" || Array.isArray(result)) {
    throw new Error("Scene result is missing its authenticated receipt");
  }
  const receipt = (result as { receipt?: unknown }).receipt;
  if (receipt === null || typeof receipt !== "object" || Array.isArray(receipt)) {
    throw new Error("Scene result contains an invalid receipt");
  }
  const captures = (receipt as { captures?: unknown }).captures;
  if (
    !Array.isArray(captures) ||
    captures.some(
      (capture) => capture === null || typeof capture !== "object" || Array.isArray(capture),
    )
  ) {
    throw new Error("Scene receipt contains an invalid captures collection");
  }
  return captures as SceneCaptureArtifact[];
}

function isDescendant(root: string, candidate: string): boolean {
  const relative = path.relative(root, candidate);
  return relative !== ""
    && relative !== ".."
    && !relative.startsWith(`..${path.sep}`)
    && !path.isAbsolute(relative);
}

async function rejectSceneReparsePath(candidate: string) {
  const root = path.parse(candidate).root;
  let current = root;
  for (const segment of path.relative(root, candidate).split(path.sep).filter(Boolean)) {
    current = path.join(current, segment);
    const info = await fs.lstat(current);
    if (info.isSymbolicLink()) {
      throw new Error(`Scene PNG path contains a symbolic link or junction: ${current}`);
    }
  }
}

async function readBoundedSceneFile(
  artifactPath: string,
  realRoot: string,
): Promise<{ bytes: Buffer; size: number }> {
  await rejectSceneReparsePath(artifactPath);
  const handle = await fs.open(artifactPath, "r");
  try {
    const before = await handle.stat({ bigint: true });
    if (
      !before.isFile() ||
      before.size < 24n ||
      before.size > BigInt(maxSceneImageBytes)
    ) {
      throw new Error("Scene PNG artifact is not a bounded regular file");
    }
    const openedRealPath = await fs.realpath(artifactPath);
    if (!isDescendant(realRoot, openedRealPath)) {
      throw new Error("Scene PNG artifact resolves outside its content-addressed root");
    }
    const openedPathInfo = await fs.stat(openedRealPath, { bigint: true });
    if (
      !openedPathInfo.isFile() ||
      openedPathInfo.dev !== before.dev ||
      openedPathInfo.ino !== before.ino ||
      openedPathInfo.size !== before.size
    ) {
      throw new Error("Scene PNG pathname does not identify the opened artifact handle");
    }

    const size = Number(before.size);
    const bytes = Buffer.allocUnsafe(size);
    let offset = 0;
    while (offset < size) {
      const read = await handle.read(bytes, offset, size - offset, offset);
      if (read.bytesRead === 0) {
        throw new Error("Scene PNG artifact was truncated while reading");
      }
      offset += read.bytesRead;
    }
    const overflow = Buffer.allocUnsafe(1);
    if ((await handle.read(overflow, 0, 1, size)).bytesRead !== 0) {
      throw new Error("Scene PNG artifact grew beyond its verified size while reading");
    }

    const after = await handle.stat({ bigint: true });
    const finalRealPath = await fs.realpath(artifactPath);
    const finalPathInfo = await fs.stat(finalRealPath, { bigint: true });
    if (
      after.dev !== before.dev ||
      after.ino !== before.ino ||
      after.size !== before.size ||
      after.mtimeNs !== before.mtimeNs ||
      finalRealPath !== openedRealPath ||
      finalPathInfo.dev !== before.dev ||
      finalPathInfo.ino !== before.ino ||
      finalPathInfo.size !== before.size
    ) {
      throw new Error("Scene PNG artifact identity changed while reading");
    }
    await rejectSceneReparsePath(artifactPath);
    return { bytes, size };
  } finally {
    await handle.close();
  }
}

async function verifiedSceneImages(
  captures: SceneCaptureArtifact[],
  sceneArtifactRoot: string,
) {
  const unique = new Map<string, SceneCaptureArtifact>();
  for (const capture of captures) {
    const artifactPath = capture.artifactPath;
    const sha256 = capture.sha256?.replace(/^sha256:/i, "").toLowerCase();
    if (
      typeof artifactPath !== "string" ||
      !path.isAbsolute(artifactPath) ||
      path.extname(artifactPath).toLowerCase() !== ".png" ||
      capture.mediaType !== "image/png" ||
      !sha256?.match(/^[0-9a-f]{64}$/)
    ) {
      throw new Error("Scene receipt contains an invalid PNG artifact identity");
    }
    const resolvedRoot = path.resolve(sceneArtifactRoot);
    const resolvedArtifact = path.resolve(artifactPath);
    const relative = path.relative(resolvedRoot, resolvedArtifact);
    const segments = relative.split(path.sep);
    if (
      !isDescendant(resolvedRoot, resolvedArtifact) ||
      segments.length !== 2 ||
      segments[0]?.toLowerCase() !== sha256.slice(0, 2) ||
      segments[1]?.toLowerCase() !== `${sha256}.png`
    ) {
      throw new Error("Scene receipt PNG path is outside its content-addressed artifact boundary");
    }
    const prior = unique.get(sha256);
    if (prior) {
      if (
        path.resolve(prior.artifactPath!) !== path.resolve(artifactPath) ||
        prior.mediaType !== capture.mediaType ||
        prior.width !== capture.width ||
        prior.height !== capture.height
      ) {
        throw new Error("Scene receipt contains conflicting duplicate PNG evidence");
      }
      continue;
    }
    unique.set(sha256, capture);
  }
  if (unique.size > maxSceneImageCount) {
    throw new Error(`Scene receipt exceeds the ${maxSceneImageCount}-image MCP attachment limit`);
  }
  if (unique.size === 0) return [];

  let totalBytes = 0;
  const realRoot = await fs.realpath(sceneArtifactRoot);
  const images: Array<{ type: "image"; data: string; mimeType: "image/png" }> = [];
  for (const [expectedHash, capture] of unique) {
    const artifactPath = capture.artifactPath!;
    const { bytes, size } = await readBoundedSceneFile(artifactPath, realRoot);
    totalBytes += size;
    if (totalBytes > maxSceneImageTotalBytes) {
      throw new Error("Scene PNG artifacts exceed the total MCP attachment byte limit");
    }
    if (
      bytes.length !== size ||
      bytes.length > maxSceneImageBytes ||
      !bytes.subarray(0, pngSignature.length).equals(pngSignature) ||
      bytes.readUInt32BE(8) !== 13 ||
      bytes.toString("ascii", 12, 16) !== "IHDR"
    ) {
      throw new Error("Scene artifact content is not a bounded PNG");
    }
    const width = bytes.readUInt32BE(16);
    const height = bytes.readUInt32BE(20);
    if (
      width === 0 ||
      height === 0 ||
      width > Math.floor(maxSceneImagePixels / height) ||
      capture.width !== width ||
      capture.height !== height
    ) {
      throw new Error("Scene PNG dimensions differ from the authenticated receipt");
    }
    const actualHash = createHash("sha256").update(bytes).digest("hex");
    if (actualHash !== expectedHash) {
      throw new Error("Scene PNG bytes differ from the authenticated receipt hash");
    }
    images.push({ type: "image", data: bytes.toString("base64"), mimeType: "image/png" });
  }
  return images;
}

async function sceneResult(
  result: unknown,
  message: string,
  includeImages = false,
  sceneArtifactRoot?: string,
) {
  const captures = sceneCaptures(result);
  const paths = captures
    .map((capture) => capture.artifactPath)
    .filter((candidate): candidate is string => Boolean(candidate));
  const pathSummary =
    paths.length === 0 ? "" : `\nInspection images:\n${paths.join("\n")}`;
  if (includeImages && !sceneArtifactRoot) {
    throw new Error("Scene image attachment root is unavailable");
  }
  const images = includeImages
    ? await verifiedSceneImages(captures, sceneArtifactRoot!)
    : [];
  return {
    content: [
      { type: "text" as const, text: `${message}${pathSummary}` },
      ...images,
    ],
    structuredContent: result as Record<string, unknown>,
  };
}

const pixelRectSchema = z.object({
  x: z.number().int().min(0),
  y: z.number().int().min(0),
  width: z.number().int().positive(),
  height: z.number().int().positive(),
});

const sceneRegionSchema = z.object({
  regionId: z.string().min(1),
  kind: z.enum(["caption", "portrait"]),
  bounds: pixelRectSchema,
  background: z.object({
    red: z.number().int().min(0).max(255),
    green: z.number().int().min(0).max(255),
    blue: z.number().int().min(0).max(255),
    alpha: z.number().int().min(0).max(255),
  }),
  colorTolerance: z.number().int().min(0).max(255).default(8),
  minForegroundPpm: z.number().int().min(0).max(1_000_000).default(1),
  minimumEdgeClearancePx: z.number().int().min(0).default(0),
});

const sha256Schema = z
  .string()
  .regex(/^(?:sha256:)?[0-9a-f]{64}$/i, "Expected a SHA-256 digest");

const nativeVoiceMutationWriteFields = {
  entityId: z.string().min(1),
  revision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  characterName: z.string().min(1),
  displayText: z.string().min(1),
  spokenText: z.string().min(1),
  frame: z.number().int().min(0),
  layer: z.number().int().min(0),
  maxLength: z.number().int().positive(),
};

const nativeVoiceMutationSchema = z
  .discriminatedUnion("action", [
    z.object({
      action: z.literal("create"),
      realizationId: z.string().uuid().optional(),
      ...nativeVoiceMutationWriteFields,
    }),
    z.object({
      action: z.literal("update"),
      realizationId: z.string().uuid(),
      ...nativeVoiceMutationWriteFields,
    }),
    z.object({
      action: z.literal("delete"),
      realizationId: z.string().uuid(),
      entityId: z.string().min(1),
      revision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
    }),
  ])
  .superRefine((value, context) => {
    if (
      value.action !== "delete" &&
      value.displayText !== value.spokenText
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        message:
          "displayText and spokenText must be identical for YMM4 native voice create/update",
        path: ["spokenText"],
      });
    }
  });

const nativeDescriptorBindingFields = {
  descriptorId: z.string().min(1),
  expectedConfigDigest: sha256Schema,
  expectedSchemaDigest: sha256Schema,
};

const nativePlacementFields = {
  frame: z.number().int().min(0),
  layer: z.number().int().min(0),
};

const nativeLossFields = {
  approvedLossyFields: z.array(z.string().min(1)).default([]),
};

const nativeEffectParameterSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("boolean"), value: z.boolean() }),
  z.object({ type: z.literal("integer"), value: z.number().int() }),
  z.object({
    type: z.literal("fixed"),
    value: z.object({
      scale: z.number().int().positive(),
      scaled: z.number().int(),
    }),
  }),
  z.object({ type: z.literal("text"), value: z.string() }),
  z.object({ type: z.literal("choice"), value: z.string() }),
  z.object({
    type: z.literal("color_rgba"),
    value: z.tuple([
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
      z.number().int().min(0).max(255),
    ]),
  }),
]);

const nativePortraitOperationSchema = (type: "portrait" | "face") =>
  z.object({
    type: z.literal(type),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    ...nativeDescriptorBindingFields,
    ...nativePlacementFields,
    durationFrames: z.number().int().positive(),
    ...nativeLossFields,
  });

const nativeAssetOperationSchema = (
  type: "image" | "video" | "audio" | "bgm",
) =>
  z.object({
    type: z.literal(type),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    sourcePath: z.string().min(1),
    artifactDigest: sha256Schema,
    mediaType: z.string().regex(/^(?:image|video|audio)\//),
    byteLength: z.number().int().positive(),
    ...nativePlacementFields,
    durationFrames: z.number().int().positive(),
    loopPlayback: z.boolean().optional(),
    ...nativeLossFields,
  });

const nativeExtensionOperationSchema = z.discriminatedUnion("type", [
  nativePortraitOperationSchema("portrait"),
  nativePortraitOperationSchema("face"),
  nativeAssetOperationSchema("image"),
  nativeAssetOperationSchema("video"),
  nativeAssetOperationSchema("audio"),
  nativeAssetOperationSchema("bgm"),
  z.object({
    type: z.literal("effect"),
    targetEntityId: z.string().min(1),
    targetEntityRevision: z.number().int().min(0),
    effectInstanceId: z.string().min(1),
    ...nativeDescriptorBindingFields,
    action: z.enum(["upsert", "remove"]),
    parameters: z.record(z.string(), nativeEffectParameterSchema).default({}),
  }),
  z.object({
    type: z.literal("template"),
    entityId: z.string().min(1),
    entityRevision: z.number().int().min(0),
    ...nativeDescriptorBindingFields,
    ...nativePlacementFields,
  }),
]);

const reconciliationDecisionSchema = z.object({
  entryId: sha256Schema,
  choice: z.enum([
    "import_into_take_graph",
    "detach_from_take_graph",
    "re_export_canonical",
  ]),
});

export function createServer(options: CreateServerOptions = {}): McpServer {
  const server = new McpServer({ name: "TakeGraph MCP", version: "0.1.0" });
  const session = options.session ?? new ProjectSession();
  const ymm4 = options.ymm4Workflow ?? new Ymm4Workflow();
  const sceneArtifactRoot =
    options.sceneArtifactRoot ?? ymm4.sceneCaptureArtifactRoot?.();
  const toolMeta = {
    ui: { resourceUri, visibility: ["model", "app"] as const },
  };

  registerAppTool(
    server,
    "studio_project_describe",
    {
      title: "Open TakeGraph Editor",
      description: "Describe the active TakeGraph project and open its editor.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
      _meta: toolMeta,
    },
    async () => {
      const state = session.snapshot();
      return stateResult(
        state,
        `TakeGraph project ${state.projectName} at revision ${state.revision}.`,
      );
    },
  );

  server.registerTool(
    "ymm4_link_describe",
    {
      title: "Describe TakeGraph YMM4 bridge",
      description:
        "Check the versioned TakeGraph YMM4 plugin, its managed capabilities, and the active project fingerprint.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
    },
    async () => {
      try {
        const result = await ymm4.describe();
        return {
          content: [
            {
              type: "text" as const,
              text: "TakeGraph YMM4 bridge and active project described.",
            },
          ],
          structuredContent: result,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_export_stage",
    {
      title: "Stage managed YMM4 utterance",
      description:
        "Generate an immutable VOICEVOX take and stage a sealed unified target plan with explicit portable_pair strategy, resolved placement, artifact binding, capability schemas, scope, and change budget without changing YMM4.",
      inputSchema: {
        entityId: z.string().min(1),
        caption: z.string().min(1),
        spokenText: z.string().min(1),
        speaker: z.string().min(1).default("春日部つむぎ"),
        style: z.string().min(1).default("ノーマル"),
        frame: z.number().int().min(0),
        audioLayer: z.number().int().min(0).default(20),
        captionLayer: z.number().int().min(0).default(21),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        const result = await ymm4.stage(input);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 export ${result.handle} staged. Approve exact digest ${result.digest} to commit.`,
            },
          ],
          structuredContent: result,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_export_commit",
    {
      title: "Commit managed YMM4 export",
      description:
        "Approve the exact staged digest, submit the sealed unified target plan directly to the bridge, verify semantic read-back, and then advance TakeGraph revision.",
      inputSchema: {
        handle: z.string().uuid(),
        digest: z.string().length(64),
      },
      annotations: { destructiveHint: true },
    },
    async ({ handle, digest }) => {
      try {
        const committed = await ymm4.commit(handle, digest);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 export ${handle} committed and verified.`,
            },
          ],
          structuredContent: committed as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_export_verify",
    {
      title: "Verify managed YMM4 export",
      description:
        "Read the active YMM4 project and verify its managed audio/caption items against a staged export.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ handle }) => {
      try {
        const verified = await ymm4.verify(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 export ${handle} matches verified read-back.`,
            },
          ],
          structuredContent: verified as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_stage",
    {
      title: "Stage native YMM4 VoiceItem",
      description:
        "Stage a sealed unified target plan with explicit native_voice strategy, resolved placement and character binding, capability schemas, scope, and change budget without changing YMM4. Display and spoken text must be exactly equal.",
      inputSchema: {
        entityId: z.string().min(1),
        displayText: z
          .string()
          .min(1)
          .describe(
            "Displayed caption text. It must exactly equal spokenText in the current native VoiceItem slice.",
          ),
        spokenText: z
          .string()
          .min(1)
          .describe(
            "Spoken text. It must exactly equal displayText in the current native VoiceItem slice.",
          ),
        characterName: z.string().min(1),
        frame: z.number().int().min(0),
        layer: z.number().int().min(0),
        maxLength: z.number().int().positive(),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        if (input.displayText !== input.spokenText) {
          throw new Error(
            "displayText and spokenText must be identical for the current YMM4 native voice slice",
          );
        }
        const result = await ymm4.stageNativeVoice(input);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native VoiceItem export ${result.handle} staged. Approve exact digest ${result.digest} to commit.`,
            },
          ],
          structuredContent: result,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_commit",
    {
      title: "Commit native YMM4 VoiceItem",
      description:
        "Approve the exact sealed plan digest, submit the unified target plan directly to the bridge, verify semantic read-back, and then advance the TakeGraph revision.",
      inputSchema: {
        handle: z.string().uuid(),
        digest: z.string().length(64),
      },
      annotations: { destructiveHint: true },
    },
    async ({ handle, digest }) => {
      try {
        const committed = await ymm4.commitNativeVoice(handle, digest);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native VoiceItem export ${handle} committed and verified.`,
            },
          ],
          structuredContent: committed as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_verify",
    {
      title: "Verify native YMM4 VoiceItem",
      description:
        "Read the active YMM4 project and verify a protocol v2 native VoiceItem realization against its staged semantic plan.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ handle }) => {
      try {
        const verified = await ymm4.verifyNativeVoice(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native VoiceItem export ${handle} matches verified semantic read-back.`,
            },
          ],
          structuredContent: verified as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_mutation_stage",
    {
      title: "Stage native YMM4 voice mutations",
      description:
        "Stage a digest-bound batch of native VoiceItem create, replacement-update with target-local state preservation, and/or delete operations. This only plans; it does not mutate YMM4.",
      inputSchema: {
        mutations: z.array(nativeVoiceMutationSchema).min(1).max(128),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        const staged = await ymm4.stageNativeVoiceMutations(input);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native voice mutation ${staged.handle} staged. Approve exact digest ${staged.digest} to commit.`,
            },
          ],
          structuredContent: staged,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_mutation_commit",
    {
      title: "Commit native YMM4 voice mutations",
      description:
        "Approve the exact mutation digest, apply the bound request, require an idempotent authenticated replay and live semantic readback, then advance the durable TakeGraph revision by CAS.",
      inputSchema: {
        handle: z.string().uuid(),
        digest: sha256Schema,
      },
      annotations: { destructiveHint: true },
    },
    async ({ handle, digest }) => {
      try {
        const committed = await ymm4.commitNativeVoiceMutations(handle, digest);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native voice mutation ${handle} committed with authenticated replay and readback.`,
            },
          ],
          structuredContent: committed as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_mutation_verify",
    {
      title: "Verify native YMM4 voice mutations",
      description:
        "Read the active YMM4 scene and verify exact create/update semantics plus absence of every approved delete realization.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ handle }) => {
      try {
        const verified = await ymm4.verifyNativeVoiceMutations(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native voice mutation ${handle} matches live semantic readback.`,
            },
          ],
          structuredContent: verified as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_voice_mutation_artifacts",
    {
      title: "Capture native YMM4 voice artifacts",
      description:
        "After durable commit, replay-authenticate the mutation and import each surviving VoiceItem's exact YMM4 WAV plus normalized host-bound voice-state provenance into TakeGraph-owned content-addressed storage. The provenance JSON is not a portable synthesis query.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: false },
    },
    async ({ handle }) => {
      try {
        const artifacts = await ymm4.captureNativeVoiceMutationArtifacts(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native voice artifacts for ${handle} were hash-verified and imported into TakeGraph CAS.`,
            },
          ],
          structuredContent: artifacts as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_stage",
    {
      title: "Stage YMM4 scene inspection",
      description:
        "Stage a digest-bound native YMM4 preview capture plan. This does not capture or change the project; a human must approve the exact digest before capture.",
      inputSchema: {
        frames: z.array(z.number().int().min(0)).min(1).max(64),
        expectedWidth: z.number().int().positive(),
        expectedHeight: z.number().int().positive(),
        profileId: z.string().min(1).default("ymm4-preview-default"),
        alpha: z.boolean().default(false),
        maxActualFrameDelta: z.number().int().min(0).default(0),
        blackLumaThreshold: z.number().int().min(0).max(255).default(16),
        blackPixelRatioPpm: z
          .number()
          .int()
          .min(1)
          .max(1_000_000)
          .default(990_000),
        blankChannelSpanThreshold: z
          .number()
          .int()
          .min(0)
          .max(255)
          .default(0),
        safeArea: pixelRectSchema.nullable().optional(),
        regions: z.array(sceneRegionSchema).default([]),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        const result = await ymm4.stageSceneInspection(input);
        const staged = result as { handle: string; digest?: string };
        return await sceneResult(
          result,
          `YMM4 scene inspection ${staged.handle} staged. A human must approve exact digest ${staged.digest ?? "(missing)"} before capture.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_approve",
    {
      title: "Approve YMM4 scene capture plan",
      description:
        "Record human approval for the exact staged scene-capture digest after rechecking the current project, scene, revision, and fingerprint.",
      inputSchema: {
        handle: z.string().uuid(),
        digest: z.string().length(64),
      },
      annotations: { destructiveHint: false },
    },
    async ({ handle, digest }) => {
      try {
        const result = await ymm4.approveSceneInspection(handle, digest);
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} approved for the exact staged digest.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_capture",
    {
      title: "Capture and ingest YMM4 scene",
      description:
        "Capture approved frames through YMM4's native PNG path, validate the authenticated receipt and authorized staging paths, import content-addressed images, and run deterministic visual checks.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: false },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.captureSceneInspection(handle);
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} captured and ingested. Automated findings are advisory; human review is still required.`,
          true,
          sceneArtifactRoot,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_review",
    {
      title: "Open human scene review",
      description:
        "Replay the authenticated native receipt, verify immutable image bytes, report semantic drift and deterministic findings, then open an explicit human review. This tool never accepts automatically.",
      inputSchema: {
        handle: z.string().uuid(),
        reviewer: z.string().min(1),
      },
      annotations: { destructiveHint: false },
    },
    async ({ handle, reviewer }) => {
      try {
        const result = await ymm4.reviewSceneInspection(handle, reviewer);
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} is ready for human review. Inspect every listed image, semantic diff, and automated finding before deciding.`,
          true,
          sceneArtifactRoot,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_decide",
    {
      title: "Accept or reject scene inspection",
      description:
        "After human image review, replay and verify evidence once more, then record an explicit accept or reject decision with an audit note. Automated findings cannot invoke this decision.",
      inputSchema: {
        handle: z.string().uuid(),
        decision: z.enum(["accept", "reject"]),
        note: z.string().min(1),
      },
      annotations: { destructiveHint: true },
    },
    async ({ handle, decision, note }) => {
      try {
        const result = await ymm4.decideSceneInspection(
          handle,
          decision,
          note,
        );
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} recorded the human decision: ${decision}.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_status",
    {
      title: "Check scene inspection status",
      description:
        "Compare a persisted inspection with the current canonical revision, scene fingerprint, and capture profile; mark it stale when any bound input changed.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: false },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.sceneInspectionStatus(handle);
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} checked.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_scene_inspection_replay",
    {
      title: "Replay scene capture receipt",
      description:
        "Reissue the same digest-bound capture operation through the authenticated bridge and re-read all staged and content-addressed PNG bytes. Persisted receipts alone are never trusted.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: false },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.replaySceneInspection(handle);
        return await sceneResult(
          result,
          `YMM4 scene inspection ${handle} receipt replayed and evidence revalidated.`,
          true,
          sceneArtifactRoot,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_project_save",
    {
      title: "Save active YMM4 project",
      description:
        "Save the active YMM4 project to its existing path after a verified managed export. This never chooses a Save As path.",
      inputSchema: {},
      annotations: { destructiveHint: true },
    },
    async () => {
      try {
        const saved = await ymm4.save();
        return {
          content: [
            {
              type: "text" as const,
              text: "The active YMM4 project was saved to its existing path.",
            },
          ],
          structuredContent: saved as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_descriptors",
    {
      title: "List YMM4 native descriptors",
      description:
        "List stable character, template, and allowlisted typed-effect descriptors with target config/schema digests and portable planning digests. Bind these exact digests when staging; display names alone are never accepted.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
    },
    async () => {
      try {
        const result = await ymm4.nativeExtensionDescriptors();
        return {
          content: [
            {
              type: "text" as const,
              text: "YMM4 native descriptors were read without changing the project.",
            },
          ],
          structuredContent: result as unknown as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_stage",
    {
      title: "Stage native YMM4 extensions",
      description:
        "Stage portraits, faces, immutable image/video/audio/BGM clips, typed effects, or native templates. The preview binds descriptor config/schema, unknown native effects, preservation evidence, artifact hashes, change budget, and the exact approvedLossyFields allowlist; it does not change YMM4.",
      inputSchema: {
        operations: z.array(nativeExtensionOperationSchema).min(1).max(64),
        maxChangedEntities: z.number().int().positive().optional(),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        const result = await ymm4.stageNativeExtension(input);
        const staged = result as { handle: string; digest?: string };
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native-extension task ${staged.handle} staged. Review warnings, preservation state, unknown effects, and every lossy field before approving exact digest ${staged.digest ?? "(missing)"}.`,
            },
          ],
          structuredContent: result,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_approve",
    {
      title: "Approve native YMM4 extension plan",
      description:
        "Recheck target fingerprint, capability schemas, descriptor config/schema, and immutable artifact bytes, then record approval for the exact staged digest. Lossy replacement is allowed only when approvedLossyFields exactly equals the driver's required list.",
      inputSchema: {
        handle: z.string().uuid(),
        digest: sha256Schema,
      },
      annotations: { destructiveHint: false },
    },
    async ({ handle, digest }) => {
      try {
        const result = await ymm4.approveNativeExtension(handle, digest);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native-extension task ${handle} approved for its exact digest.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_apply",
    {
      title: "Apply native YMM4 extension plan",
      description:
        "Apply an already approved native-extension task exactly once, verify request-bound semantic read-back including unknown-effect preservation, and advance canonical revision only after verification.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: true },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.applyNativeExtension(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native-extension task ${handle} applied and verified.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_verify",
    {
      title: "Verify current native YMM4 extensions",
      description:
        "Replay the authenticated receipt, rehash immutable artifacts, then independently re-observe current YMM4 realizations and unknown native-effect state. A persisted receipt alone is not trusted.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.verifyNativeExtension(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native-extension task ${handle} matches current semantic read-back.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_native_extension_status",
    {
      title: "Check native YMM4 extension status",
      description:
        "Revalidate a staged task or replay its request-bound receipt and report current semantic verification, descriptor drift, artifact drift, warnings, and recovery status.",
      inputSchema: { handle: z.string().uuid() },
      annotations: { destructiveHint: false },
    },
    async ({ handle }) => {
      try {
        const result = await ymm4.nativeExtensionStatus(handle);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 native-extension task ${handle} status checked.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_checkpoint_stage",
    {
      title: "Stage verified YMM4 checkpoint",
      description:
        "Stage an existing-path-only YMM4 save checkpoint bound to the current canonical revision, target identity, scene fingerprint, and checkpoint driver profile. This does not save or advance the canonical revision.",
      inputSchema: {},
      annotations: { destructiveHint: false },
    },
    async () => {
      try {
        const result = await ymm4.stageCheckpoint();
        const operationId = (result as { payload?: { request?: { operationId?: string } } })
          .payload?.request?.operationId;
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 checkpoint ${operationId ?? "(unknown)"} staged. Execute it to save the existing project path and verify file evidence.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_checkpoint_execute",
    {
      title: "Execute verified YMM4 checkpoint",
      description:
        "Execute or idempotently resume a staged existing-path checkpoint. The workflow rechecks source bindings, invokes YMM4 save, hashes the project file, verifies unchanged semantic state, and never advances the canonical revision.",
      inputSchema: { operationId: z.string().uuid() },
      annotations: { destructiveHint: true },
    },
    async ({ operationId }) => {
      try {
        const result = await ymm4.executeCheckpoint(operationId);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 checkpoint ${operationId} executed; inspect its durable status and file receipt.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_checkpoint_status",
    {
      title: "Read YMM4 checkpoint status",
      description:
        "Read and validate the latest append-only checkpoint generation, including source bindings, project-file hashes, verification, staleness, failure, or recovery-required state.",
      inputSchema: { operationId: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ operationId }) => {
      try {
        const result = await ymm4.checkpointStatus(operationId);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 checkpoint ${operationId} status read from the durable journal.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_render_profiles",
    {
      title: "List YMM4 render profiles",
      description:
        "List exact bridge-supported YMM4 render profile descriptors and their digest-bound dimensions, frame rate, audio setting, container, and driver profile.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
    },
    async () => {
      try {
        const result = await ymm4.renderProfiles();
        return {
          content: [
            {
              type: "text" as const,
              text: "YMM4 render profile descriptors read without changing the project.",
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_render_stage",
    {
      title: "Stage authoritative YMM4 render",
      description:
        "Stage a render bound to a verified checkpoint operation, its exact saved file hash/length, the current canonical revision, target fingerprint, and exact render profile. outputPath must be absolute; an existing file can be replaced only when overwrite is explicitly true. Staging does not start rendering.",
      inputSchema: {
        checkpointOperationId: z.string().uuid(),
        profile: z.string().min(1),
        outputPath: z.string().min(1),
        overwrite: z.boolean().default(false),
      },
      annotations: { destructiveHint: false },
    },
    async (input) => {
      try {
        const result = await ymm4.stageRender(input);
        const taskId = (result as { payload?: { request?: { taskId?: string } } })
          .payload?.request?.taskId;
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 render ${taskId ?? "(unknown)"} staged for explicit execution.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_render_execute",
    {
      title: "Execute or poll YMM4 render",
      description:
        "Submit a staged render or poll its persisted lifecycle once. Success is accepted only after independent MP4 hashing and media probing; the render never advances the canonical revision.",
      inputSchema: { taskId: z.string().uuid() },
      annotations: { destructiveHint: true },
    },
    async ({ taskId }) => {
      try {
        const result = await ymm4.executeRender(taskId);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 render ${taskId} submitted or polled once; inspect the returned durable lifecycle.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_render_status",
    {
      title: "Read YMM4 render status",
      description:
        "Read and validate the latest append-only render generation, including progress, cancellation, staleness, recovery state, and verified final-media evidence when complete.",
      inputSchema: { taskId: z.string().uuid() },
      annotations: { readOnlyHint: true },
    },
    async ({ taskId }) => {
      try {
        const result = await ymm4.renderStatus(taskId);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 render ${taskId} status read from the durable journal.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_render_cancel",
    {
      title: "Cancel YMM4 render",
      description:
        "Request cooperative cancellation for one persisted render task. The durable result distinguishes cancelling, cancelled, completed, failed, stale, and recovery-required outcomes.",
      inputSchema: { taskId: z.string().uuid() },
      annotations: { destructiveHint: true },
    },
    async ({ taskId }) => {
      try {
        const result = await ymm4.cancelRender(taskId);
        return {
          content: [
            {
              type: "text" as const,
              text: `Cancellation requested for YMM4 render ${taskId}.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_report",
    {
      title: "Report managed YMM4 semantic drift",
      description:
        "Compare receipt-derived durable managed semantics with a fresh YMM4 observation. Only TakeGraph-owned fields participate; unmanaged items and preserved native fields remain outside the report. Callers cannot supply the canonical expectation, and staging does not change either side.",
      inputSchema: {},
      annotations: { destructiveHint: false },
    },
    async () => {
      try {
        const result = await ymm4.reconciliationReport();
        const reportDigest = (result as { payload?: { report?: { reportDigest?: string } } })
          .payload?.report?.reportDigest;
        return {
          content: [
            {
              type: "text" as const,
              text: `Managed YMM4 drift report ${reportDigest ?? "(unknown)"} staged. Every entry requires an explicit reconciliation decision.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_preview",
    {
      title: "Preview explicit YMM4 reconciliation",
      description:
        "Bind exactly one import, detach, or re-export choice to every immutable drift entry and produce an approval digest. Previewing does not import, detach, re-export, or advance the canonical revision.",
      inputSchema: {
        reportDigest: sha256Schema,
        decisions: z.array(reconciliationDecisionSchema).max(4096),
      },
      annotations: { destructiveHint: false },
    },
    async ({ reportDigest, decisions }) => {
      try {
        const result = await ymm4.previewReconciliation(
          reportDigest,
          decisions,
        );
        const approvalDigest = (result as { payload?: { preview?: { approvalDigest?: string } } })
          .payload?.preview?.approvalDigest;
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 reconciliation preview staged. Review every action before accepting exact digest ${approvalDigest ?? "(unknown)"}.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_apply",
    {
      title: "Accept YMM4 reconciliation actions",
      description:
        "After a fresh target read-back, accept the exact digest-bound reconciliation preview. This records explicit import/detach/re-export actions for their normal guarded workflows; it never silently mutates YMM4 or canonical project state.",
      inputSchema: {
        reportDigest: sha256Schema,
        approvalDigest: sha256Schema,
      },
      annotations: { destructiveHint: true },
    },
    async ({ reportDigest, approvalDigest }) => {
      try {
        const result = await ymm4.applyReconciliation(
          reportDigest,
          approvalDigest,
        );
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 reconciliation actions for report ${reportDigest} accepted without silent synchronization.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_child_status",
    {
      title: "Read YMM4 reconciliation child",
      description:
        "Read the hash-chained downstream child created from an accepted reconciliation action, including whether it still awaits its independent approval.",
      inputSchema: { childTaskId: sha256Schema },
      annotations: { readOnlyHint: true, destructiveHint: false },
    },
    async ({ childTaskId }) => {
      try {
        const result = await ymm4.reconciliationChildStatus(childTaskId);
        return {
          content: [
            {
              type: "text" as const,
              text: `YMM4 reconciliation child ${childTaskId} loaded.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_detach_approve",
    {
      title: "Approve YMM4 metadata detach",
      description:
        "Independently approve the exact digest of a metadata-only Remark detach child. Reconciliation approval is not reused, and this step does not mutate YMM4.",
      inputSchema: {
        childTaskId: sha256Schema,
        approvalDigest: sha256Schema,
      },
      annotations: { destructiveHint: false },
    },
    async ({ childTaskId, approvalDigest }) => {
      try {
        const result = await ymm4.approveReconciliationDetach(
          childTaskId,
          approvalDigest,
        );
        return {
          content: [
            {
              type: "text" as const,
              text: `Metadata detach ${childTaskId} approved for its exact current digest. YMM4 has not been changed.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_detach_execute",
    {
      title: "Execute YMM4 metadata detach",
      description:
        "Execute or idempotently replay an independently approved permanent Remark-only detach. The bridge WAL binds the full before/expected-post scene Remark set and proves exact fresh read-back plus unchanged non-Remark content; the service then CAS-removes canonical ownership and advances one revision.",
      inputSchema: { childTaskId: sha256Schema },
      annotations: { destructiveHint: true },
    },
    async ({ childTaskId }) => {
      try {
        const result = await ymm4.executeReconciliationDetach(childTaskId);
        return {
          content: [
            {
              type: "text" as const,
              text: `Metadata detach ${childTaskId} executed, verified by exact fresh YMM4 read-back, and permanently removed from canonical ownership.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  server.registerTool(
    "ymm4_reconcile_re_export_dispatch",
    {
      title: "Stage canonical YMM4 re-export",
      description:
        "Validate a route-tagged exporter manifest against the durable canonical projection, then invoke the existing portable-pair, native-voice-mutation, or native-extension staging path. The returned handle and digest address that exporter's saved, unapproved task for its normal approval/apply tools; this tool never approves or applies it.",
      inputSchema: {
        childTaskId: sha256Schema,
        manifest: z.record(z.string(), z.unknown()),
      },
      annotations: { destructiveHint: false },
    },
    async ({ childTaskId, manifest }) => {
      try {
        const result = await ymm4.dispatchReconciliationReExport(
          childTaskId,
          manifest,
        );
        return {
          content: [
            {
              type: "text" as const,
              text: `Canonical re-export ${childTaskId} was saved as a new existing-exporter preview. Use its returned handle and digest with the route-specific approval/apply tools; no YMM4 mutation or approval occurred.`,
            },
          ],
          structuredContent: result as Record<string, unknown>,
        };
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppTool(
    server,
    "studio_ui_get_state",
    {
      title: "Refresh TakeGraph state",
      description: "Return the latest TakeGraph editor state to its app view.",
      inputSchema: {},
      annotations: { readOnlyHint: true },
      _meta: {
        ui: { resourceUri, visibility: ["app"] as const },
      },
    },
    async () => stateResult(session.snapshot(), "TakeGraph state refreshed."),
  );

  registerAppTool(
    server,
    "voice_generate_variant",
    {
      title: "Create voice-take candidate",
      description:
        "Create a new immutable VoiceTake candidate and capture its synthesis settings.",
      inputSchema: {
        utteranceId: z.string().min(1),
        speed: z.number().min(0.5).max(2),
        intonation: z.number().min(0).max(2),
      },
      annotations: { destructiveHint: false },
      _meta: toolMeta,
    },
    async (input) => {
      try {
        return stateResult(
          session.generateVariant(input),
          "A new VoiceTake candidate was created. Its synthesis query is ready; the audio artifact is not complete.",
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppTool(
    server,
    "voice_stage_take_patch",
    {
      title: "Preview take adoption",
      description:
        "Stage a digest-bound patch that previews adopting an existing ready VoiceTake.",
      inputSchema: { takeId: z.string().min(1) },
      annotations: { destructiveHint: false },
      _meta: toolMeta,
    },
    async ({ takeId }) => {
      try {
        return stateResult(
          session.stageTake(takeId),
          `A previewable patch for ${takeId} was staged.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppTool(
    server,
    "studio_patch_commit",
    {
      title: "Approve and commit take patch",
      description:
        "Approve the exact staged digest and commit it only if its base revision is current.",
      inputSchema: {
        patchId: z.string().min(1),
        digest: z.string().length(64),
      },
      annotations: { destructiveHint: true },
      _meta: toolMeta,
    },
    async (input) => {
      try {
        const state = await session.commitPatch(input);
        return stateResult(
          state,
          `Patch committed. Project revision is now ${state.revision}.`,
        );
      } catch (error) {
        return errorResult(error);
      }
    },
  );

  registerAppResource(
    server,
    "TakeGraph Editor",
    resourceUri,
    { mimeType: RESOURCE_MIME_TYPE },
    async () => {
      const html =
        options.viewHtml ??
        (await fs.readFile(path.join(viewDirectory, "mcp-app.html"), "utf8"));
      return {
        contents: [
          { uri: resourceUri, mimeType: RESOURCE_MIME_TYPE, text: html },
        ],
      };
    },
  );

  return server;
}
