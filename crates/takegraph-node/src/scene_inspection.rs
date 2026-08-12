//! Deterministic PNG ingestion and visual scene checks.
//!
//! This module deliberately does not capture a YMM4 window. A version-specific
//! bridge supplies PNG files in an authorized staging directory; the media node
//! decodes them, computes their content identity, runs reproducible pixel checks,
//! and imports the bytes into immutable artifact storage.

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::YMM4_BRIDGE_PROTOCOL_VERSION;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_CAPTURE_PIXELS: u64 = 100_000_000;
const PARTS_PER_MILLION: u64 = 1_000_000;
const PARTS_PER_MILLION_U32: u32 = 1_000_000;

/// The only authoritative still-image format in the first inspection slice.
pub const PNG_MEDIA_TYPE: &str = "image/png";
/// Version bound into every service-side capture-profile digest.
pub const SCENE_PIXEL_DETECTOR_VERSION: &str = "takegraph-pixel-checks/v1";
/// Pinned native capture driver for the currently tested YMM4 runtime.
pub const YMM4_SCENE_CAPTURE_DRIVER_ID: &str = "ymm4-preview-save-image/4.55.1.1";
/// SHA-256 of [`YMM4_SCENE_CAPTURE_DRIVER_ID`], returned by the bridge receipt.
pub const YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST: &str =
    "1e7346f55168bd169fd8a6850ab8cadc9c74ee7622d7a8ae4ca63aade0aab3a9";

/// Digest-bound request for a native, chrome-free YMM4 scene capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4SceneCaptureRequest {
    pub protocol_version: u32,
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub source_revision: u64,
    pub capture_profile_digest: String,
    pub frames: Vec<u32>,
    pub alpha: bool,
}

/// Fields from an approved scene plan used to build a capture request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ymm4SceneCaptureRequestInput {
    pub operation_id: Uuid,
    pub project_id: String,
    pub scene_id: String,
    pub expected_fingerprint: String,
    pub source_revision: u64,
    pub capture_profile_digest: String,
    pub frames: Vec<u32>,
    pub alpha: bool,
}

impl Ymm4SceneCaptureRequest {
    /// Builds a request with sorted, de-duplicated frames and its canonical hash.
    #[must_use]
    pub fn new(input: Ymm4SceneCaptureRequestInput) -> Self {
        let frames: Vec<_> = input
            .frames
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut request = Self {
            protocol_version: YMM4_BRIDGE_PROTOCOL_VERSION,
            operation_id: input.operation_id,
            request_digest: String::new(),
            project_id: input.project_id,
            scene_id: input.scene_id,
            expected_fingerprint: input.expected_fingerprint,
            source_revision: input.source_revision,
            capture_profile_digest: input.capture_profile_digest,
            frames,
            alpha: input.alpha,
        };
        request.request_digest = scene_capture_request_digest(&request);
        request
    }
}

fn scene_capture_request_digest(request: &Ymm4SceneCaptureRequest) -> String {
    fn write_string(canonical: &mut String, label: &str, value: &str) {
        use std::fmt::Write as _;
        let _ = writeln!(canonical, "{label}:{}:{value}", value.len());
    }

    use std::fmt::Write as _;
    let mut canonical = String::from("takegraph-ymm4-scene-capture-v2\n");
    let _ = writeln!(canonical, "protocolVersion:{}", request.protocol_version);
    write_string(
        &mut canonical,
        "operationId",
        &request.operation_id.hyphenated().to_string(),
    );
    write_string(&mut canonical, "projectId", &request.project_id);
    write_string(&mut canonical, "sceneId", &request.scene_id);
    write_string(
        &mut canonical,
        "expectedFingerprint",
        &request.expected_fingerprint,
    );
    let _ = writeln!(canonical, "sourceRevision:{}", request.source_revision);
    write_string(
        &mut canonical,
        "captureProfileDigest",
        &request.capture_profile_digest,
    );
    let _ = writeln!(canonical, "alpha:{}", u8::from(request.alpha));
    let _ = writeln!(canonical, "frames:{}", request.frames.len());
    for frame in &request.frames {
        let _ = writeln!(canonical, "frame:{frame}");
    }
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

/// One native bridge capture claim. The node re-reads every claimed property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4SceneCaptureFrameReceipt {
    pub requested_frame: u32,
    pub actual_frame: u32,
    pub path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub media_type: String,
}

/// Native capture outcome. This is evidence only after authenticated transport
/// plus artifact read-back by `takegraph-service`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4SceneCaptureReceipt {
    pub operation_id: Uuid,
    pub request_digest: String,
    pub project_id: String,
    pub scene_id: String,
    pub source_revision: u64,
    pub expected_fingerprint: String,
    pub capture_profile_digest: String,
    pub status: Ymm4SceneCaptureStatus,
    pub before_fingerprint: String,
    pub after_fingerprint: String,
    pub frames: Vec<Ymm4SceneCaptureFrameReceipt>,
    pub driver: String,
    pub driver_profile_digest: String,
    pub transient_state_restored: bool,
    pub project_dirty_before: bool,
    pub project_dirty_after: bool,
    pub error: Option<String>,
}

/// Status values produced by the current native capture driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4SceneCaptureStatus {
    Captured,
    Stale,
}

/// An integer pixel rectangle, measured from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    fn right(self) -> Option<u32> {
        self.x.checked_add(self.width)
    }

    fn bottom(self) -> Option<u32> {
        self.y.checked_add(self.height)
    }

    fn is_within(self, width: u32, height: u32) -> bool {
        self.width > 0
            && self.height > 0
            && self.right().is_some_and(|right| right <= width)
            && self.bottom().is_some_and(|bottom| bottom <= height)
    }

    fn contains(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self
                .right()
                .zip(other.right())
                .is_some_and(|(right, other_right)| right >= other_right)
            && self
                .bottom()
                .zip(other.bottom())
                .is_some_and(|(bottom, other_bottom)| bottom >= other_bottom)
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right()?.min(other.right()?);
        let bottom = self.bottom()?.min(other.bottom()?);
        (right > left && bottom > top).then_some(Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

/// An eight-bit RGBA color used as a deterministic region background reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rgba8 {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

/// Semantic role of a pixel region expected in a rendered scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualRegionKind {
    Caption,
    Portrait,
}

/// Pixel-level expectation supplied by the capture profile.
//
/// A pixel is foreground when any RGBA channel differs from `background` by
/// more than `color_tolerance`. This is intentionally simpler and more stable
/// than OCR or image-model inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualRegionExpectation {
    pub region_id: String,
    pub kind: VisualRegionKind,
    pub bounds: PixelRect,
    pub background: Rgba8,
    pub color_tolerance: u8,
    /// Minimum foreground coverage, in parts per million of the region.
    pub min_foreground_ppm: u32,
    /// Minimum clear pixels required between detected content and each edge.
    /// Zero disables edge-clipping detection.
    pub minimum_edge_clearance_px: u32,
}

/// Deterministic still-frame checks bound into a capture plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneVisualCheckProfile {
    pub expected_width: u32,
    pub expected_height: u32,
    pub black_luma_threshold: u8,
    /// Minimum dark, visible coverage for a black-frame finding.
    pub black_pixel_ratio_ppm: u32,
    /// A frame with no visible pixels or an RGBA channel span at or below this
    /// value is considered visually blank/uniform.
    pub blank_channel_span_threshold: u8,
    pub safe_area: Option<PixelRect>,
    pub regions: Vec<VisualRegionExpectation>,
}

impl SceneVisualCheckProfile {
    /// Validates dimensions, detector ratios, region IDs, and rectangle bounds.
    ///
    /// # Errors
    ///
    /// Returns an error for an impossible or ambiguous inspection profile.
    pub fn validate(&self) -> Result<(), SceneInspectionNodeError> {
        if self.expected_width == 0 || self.expected_height == 0 {
            return Err(SceneInspectionNodeError::InvalidProfile(
                "expected dimensions must be non-zero".into(),
            ));
        }
        let pixels = u64::from(self.expected_width) * u64::from(self.expected_height);
        if pixels > MAX_CAPTURE_PIXELS {
            return Err(SceneInspectionNodeError::ImageTooLarge {
                width: self.expected_width,
                height: self.expected_height,
            });
        }
        if self.black_pixel_ratio_ppm == 0 || self.black_pixel_ratio_ppm > PARTS_PER_MILLION_U32 {
            return Err(SceneInspectionNodeError::InvalidProfile(
                "blackPixelRatioPpm must be in 1..=1000000".into(),
            ));
        }
        if self
            .safe_area
            .is_some_and(|area| !area.is_within(self.expected_width, self.expected_height))
        {
            return Err(SceneInspectionNodeError::InvalidProfile(
                "safeArea must be inside the expected image".into(),
            ));
        }

        let mut region_ids = std::collections::BTreeSet::new();
        for region in &self.regions {
            if region.region_id.trim().is_empty() || !region_ids.insert(&region.region_id) {
                return Err(SceneInspectionNodeError::InvalidProfile(
                    "visual region IDs must be non-empty and unique".into(),
                ));
            }
            if region.min_foreground_ppm > PARTS_PER_MILLION_U32 {
                return Err(SceneInspectionNodeError::InvalidProfile(format!(
                    "region {} minForegroundPpm must not exceed 1000000",
                    region.region_id
                )));
            }
            if !region
                .bounds
                .is_within(self.expected_width, self.expected_height)
            {
                return Err(SceneInspectionNodeError::InvalidProfile(format!(
                    "region {} must be inside the expected image",
                    region.region_id
                )));
            }
        }
        Ok(())
    }
}

/// Stable code for a deterministic inspection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneFindingCode {
    DimensionsMismatch,
    BlankFrame,
    BlackFrame,
    CaptionMissing,
    CaptionClipped,
    CaptionOutsideSafeArea,
    PortraitMissing,
    PortraitClipped,
    PortraitOutsideSafeArea,
    CaptionPortraitOverlap,
}

/// Triage level. All still-image findings remain advisory to the canonical edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneFindingSeverity {
    Warning,
    Error,
}

/// One reproducible finding from the decoded pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneInspectionFinding {
    pub code: SceneFindingCode,
    pub severity: SceneFindingSeverity,
    pub region_ids: Vec<String>,
    pub message: String,
}

/// Measured result for an expected visual region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualRegionDetection {
    pub region_id: String,
    pub kind: VisualRegionKind,
    pub foreground_pixels: u64,
    pub foreground_ppm: u32,
    pub detected_bounds: Option<PixelRect>,
}

/// Pixel statistics and findings for one decoded frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneFrameInspection {
    pub width: u32,
    pub height: u32,
    pub visible_pixels: u64,
    pub dark_visible_pixels: u64,
    pub dark_visible_ppm: u32,
    pub detections: Vec<VisualRegionDetection>,
    pub findings: Vec<SceneInspectionFinding>,
    /// Identifies the deterministic algorithm used for all measurements.
    pub detector_version: String,
}

/// Immutable, content-addressed PNG plus its read-back inspection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedSceneCapture {
    pub sample_id: Uuid,
    pub requested_frame: u32,
    pub actual_frame: u32,
    pub sha256: String,
    pub artifact_path: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub inspection: SceneFrameInspection,
}

#[derive(Debug)]
struct DecodedPng {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Imports and verifies one staged PNG capture by content identity.
///
/// The function decodes before import, writes beneath
/// `artifact_root/scene-captures`, reads the stored bytes back, checks their
/// hash, and decodes them again before returning a receipt.
///
/// # Errors
///
/// Returns an error for malformed/oversized PNG data, an invalid profile,
/// filesystem failure, or an immutable-artifact collision.
pub fn import_png_capture(
    source_path: &Path,
    artifact_root: &Path,
    sample_id: Uuid,
    requested_frame: u32,
    actual_frame: u32,
    profile: &SceneVisualCheckProfile,
) -> Result<ImportedSceneCapture, SceneInspectionNodeError> {
    profile.validate()?;
    let source_bytes = fs::read(source_path)?;
    let decoded_source = decode_png(&source_bytes)?;
    let hash = sha256(&source_bytes);
    let directory = artifact_root.join("scene-captures").join(&hash[..2]);
    fs::create_dir_all(&directory)?;
    let artifact_path = directory.join(format!("{hash}.png"));
    write_immutable(&artifact_path, &source_bytes, &hash)?;

    let stored_bytes = fs::read(&artifact_path)?;
    let stored_hash = sha256(&stored_bytes);
    if stored_hash != hash {
        return Err(SceneInspectionNodeError::ArtifactReadbackMismatch {
            expected: hash,
            actual: stored_hash,
        });
    }
    let decoded_stored = decode_png(&stored_bytes)?;
    if decoded_stored.width != decoded_source.width
        || decoded_stored.height != decoded_source.height
        || decoded_stored.rgba != decoded_source.rgba
    {
        return Err(SceneInspectionNodeError::ArtifactDecodeMismatch);
    }
    let inspection = inspect_rgba(&decoded_stored, profile);

    Ok(ImportedSceneCapture {
        sample_id,
        requested_frame,
        actual_frame,
        sha256: stored_hash,
        artifact_path: artifact_path.to_string_lossy().into_owned(),
        media_type: PNG_MEDIA_TYPE.into(),
        width: decoded_stored.width,
        height: decoded_stored.height,
        inspection,
    })
}

/// Re-reads an imported artifact and proves its identity and PNG dimensions.
///
/// # Errors
///
/// Returns an error if the file disappeared, changed, or no longer decodes to
/// the receipt dimensions.
pub fn verify_imported_capture(
    capture: &ImportedSceneCapture,
) -> Result<(), SceneInspectionNodeError> {
    let bytes = fs::read(&capture.artifact_path)?;
    let actual_hash = sha256(&bytes);
    if actual_hash != capture.sha256 {
        return Err(SceneInspectionNodeError::ArtifactReadbackMismatch {
            expected: capture.sha256.clone(),
            actual: actual_hash,
        });
    }
    let decoded = decode_png(&bytes)?;
    if decoded.width != capture.width || decoded.height != capture.height {
        return Err(SceneInspectionNodeError::ArtifactDecodeMismatch);
    }
    Ok(())
}

fn decode_png(bytes: &[u8]) -> Result<DecodedPng, SceneInspectionNodeError> {
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(SceneInspectionNodeError::NotPng);
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let width = reader.info().width;
    let height = reader.info().height;
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || pixels > MAX_CAPTURE_PIXELS {
        return Err(SceneInspectionNodeError::ImageTooLarge { width, height });
    }

    let mut output = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut output)?;
    output.truncate(info.buffer_size());
    let rgba = normalize_rgba(&output, info.color_type)?;
    let expected_len = usize::try_from(pixels)
        .ok()
        .and_then(|count| count.checked_mul(4))
        .ok_or(SceneInspectionNodeError::ImageTooLarge { width, height })?;
    if rgba.len() != expected_len {
        return Err(SceneInspectionNodeError::UnexpectedPixelBuffer);
    }
    Ok(DecodedPng {
        width,
        height,
        rgba,
    })
}

fn normalize_rgba(
    source: &[u8],
    color_type: png::ColorType,
) -> Result<Vec<u8>, SceneInspectionNodeError> {
    let channels = match color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(SceneInspectionNodeError::UnexpectedIndexedPng),
    };
    if !source.len().is_multiple_of(channels) {
        return Err(SceneInspectionNodeError::UnexpectedPixelBuffer);
    }
    let mut rgba = Vec::with_capacity((source.len() / channels) * 4);
    for pixel in source.chunks_exact(channels) {
        match color_type {
            png::ColorType::Grayscale => {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]);
            }
            png::ColorType::GrayscaleAlpha => {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            png::ColorType::Rgb => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            png::ColorType::Rgba => rgba.extend_from_slice(pixel),
            png::ColorType::Indexed => unreachable!("indexed PNG was expanded by decoder"),
        }
    }
    Ok(rgba)
}

fn inspect_rgba(decoded: &DecodedPng, profile: &SceneVisualCheckProfile) -> SceneFrameInspection {
    let total_pixels = u64::from(decoded.width) * u64::from(decoded.height);
    let mut visible_pixels = 0u64;
    let mut dark_visible_pixels = 0u64;
    let mut minimum = [u8::MAX; 4];
    let mut maximum = [u8::MIN; 4];

    for pixel in decoded.rgba.chunks_exact(4) {
        for channel in 0..4 {
            minimum[channel] = minimum[channel].min(pixel[channel]);
            maximum[channel] = maximum[channel].max(pixel[channel]);
        }
        if pixel[3] > 0 {
            visible_pixels += 1;
            if luma(pixel[0], pixel[1], pixel[2]) <= u32::from(profile.black_luma_threshold) {
                dark_visible_pixels += 1;
            }
        }
    }

    let dark_visible_ppm = ppm(dark_visible_pixels, total_pixels);
    let mut findings = Vec::new();
    if decoded.width != profile.expected_width || decoded.height != profile.expected_height {
        findings.push(finding(
            SceneFindingCode::DimensionsMismatch,
            SceneFindingSeverity::Error,
            Vec::new(),
            format!(
                "expected {}x{}, captured {}x{}",
                profile.expected_width, profile.expected_height, decoded.width, decoded.height
            ),
        ));
    }

    let maximum_span = (0..4)
        .map(|channel| maximum[channel].saturating_sub(minimum[channel]))
        .max()
        .unwrap_or(0);
    if visible_pixels == 0 || maximum_span <= profile.blank_channel_span_threshold {
        findings.push(finding(
            SceneFindingCode::BlankFrame,
            SceneFindingSeverity::Error,
            Vec::new(),
            "frame is transparent or visually uniform".into(),
        ));
    }
    if dark_visible_ppm >= profile.black_pixel_ratio_ppm {
        findings.push(finding(
            SceneFindingCode::BlackFrame,
            SceneFindingSeverity::Error,
            Vec::new(),
            format!("{dark_visible_ppm} ppm of the frame is dark and visible"),
        ));
    }

    let mut detections = Vec::with_capacity(profile.regions.len());
    for expected in &profile.regions {
        let detection = detect_region(decoded, expected);
        append_region_findings(&detection, expected, profile.safe_area, &mut findings);
        detections.push(detection);
    }
    append_overlap_findings(&detections, &mut findings);

    SceneFrameInspection {
        width: decoded.width,
        height: decoded.height,
        visible_pixels,
        dark_visible_pixels,
        dark_visible_ppm,
        detections,
        findings,
        detector_version: SCENE_PIXEL_DETECTOR_VERSION.into(),
    }
}

fn detect_region(
    decoded: &DecodedPng,
    expected: &VisualRegionExpectation,
) -> VisualRegionDetection {
    let image_bounds = PixelRect {
        x: 0,
        y: 0,
        width: decoded.width,
        height: decoded.height,
    };
    let Some(bounds) = expected.bounds.intersection(image_bounds) else {
        return VisualRegionDetection {
            region_id: expected.region_id.clone(),
            kind: expected.kind,
            foreground_pixels: 0,
            foreground_ppm: 0,
            detected_bounds: None,
        };
    };

    let mut foreground_pixels = 0u64;
    let mut left = u32::MAX;
    let mut top = u32::MAX;
    let mut right = 0u32;
    let mut bottom = 0u32;
    for y in bounds.y..bounds.bottom().unwrap_or(bounds.y) {
        for x in bounds.x..bounds.right().unwrap_or(bounds.x) {
            let Some(offset) = pixel_offset(decoded.width, x, y) else {
                continue;
            };
            let pixel = &decoded.rgba[offset..offset + 4];
            if is_foreground(pixel, expected.background, expected.color_tolerance) {
                foreground_pixels += 1;
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    let detected_bounds = if foreground_pixels > 0 {
        Some(PixelRect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    } else {
        None
    };
    let region_pixels = u64::from(expected.bounds.width) * u64::from(expected.bounds.height);
    VisualRegionDetection {
        region_id: expected.region_id.clone(),
        kind: expected.kind,
        foreground_pixels,
        foreground_ppm: ppm(foreground_pixels, region_pixels),
        detected_bounds,
    }
}

fn append_region_findings(
    detection: &VisualRegionDetection,
    expected: &VisualRegionExpectation,
    safe_area: Option<PixelRect>,
    findings: &mut Vec<SceneInspectionFinding>,
) {
    let (missing_code, clipped_code, outside_code, name) = match expected.kind {
        VisualRegionKind::Caption => (
            SceneFindingCode::CaptionMissing,
            SceneFindingCode::CaptionClipped,
            SceneFindingCode::CaptionOutsideSafeArea,
            "caption",
        ),
        VisualRegionKind::Portrait => (
            SceneFindingCode::PortraitMissing,
            SceneFindingCode::PortraitClipped,
            SceneFindingCode::PortraitOutsideSafeArea,
            "portrait",
        ),
    };
    if detection.foreground_ppm < expected.min_foreground_ppm {
        findings.push(finding(
            missing_code,
            SceneFindingSeverity::Warning,
            vec![expected.region_id.clone()],
            format!(
                "{name} foreground is {} ppm, below the required {} ppm",
                detection.foreground_ppm, expected.min_foreground_ppm
            ),
        ));
        return;
    }

    if let Some(bounds) = detection.detected_bounds {
        if expected.minimum_edge_clearance_px > 0
            && !has_edge_clearance(expected.bounds, bounds, expected.minimum_edge_clearance_px)
        {
            findings.push(finding(
                clipped_code,
                SceneFindingSeverity::Warning,
                vec![expected.region_id.clone()],
                format!("{name} foreground reaches the configured region edge"),
            ));
        }
        if safe_area.is_some_and(|area| !area.contains(bounds)) {
            findings.push(finding(
                outside_code,
                SceneFindingSeverity::Warning,
                vec![expected.region_id.clone()],
                format!("{name} foreground extends outside the configured safe area"),
            ));
        }
    }
}

fn append_overlap_findings(
    detections: &[VisualRegionDetection],
    findings: &mut Vec<SceneInspectionFinding>,
) {
    for (index, left) in detections.iter().enumerate() {
        let Some(left_bounds) = left.detected_bounds else {
            continue;
        };
        for right in &detections[index + 1..] {
            if left.kind == right.kind {
                continue;
            }
            let Some(right_bounds) = right.detected_bounds else {
                continue;
            };
            if left_bounds.intersection(right_bounds).is_some() {
                findings.push(finding(
                    SceneFindingCode::CaptionPortraitOverlap,
                    SceneFindingSeverity::Warning,
                    vec![left.region_id.clone(), right.region_id.clone()],
                    "detected caption and portrait foreground bounds overlap".into(),
                ));
            }
        }
    }
}

fn has_edge_clearance(container: PixelRect, content: PixelRect, clearance: u32) -> bool {
    let Some(container_right) = container.right() else {
        return false;
    };
    let Some(container_bottom) = container.bottom() else {
        return false;
    };
    let Some(content_right) = content.right() else {
        return false;
    };
    let Some(content_bottom) = content.bottom() else {
        return false;
    };
    content.x.saturating_sub(container.x) >= clearance
        && content.y.saturating_sub(container.y) >= clearance
        && container_right.saturating_sub(content_right) >= clearance
        && container_bottom.saturating_sub(content_bottom) >= clearance
}

fn is_foreground(pixel: &[u8], background: Rgba8, tolerance: u8) -> bool {
    let reference = [
        background.red,
        background.green,
        background.blue,
        background.alpha,
    ];
    pixel
        .iter()
        .zip(reference)
        .any(|(actual, expected)| actual.abs_diff(expected) > tolerance)
}

fn luma(red: u8, green: u8, blue: u8) -> u32 {
    // Integer Rec. 709 approximation in the original 0..=255 range.
    (54 * u32::from(red) + 183 * u32::from(green) + 19 * u32::from(blue)) / 256
}

fn ppm(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        return 0;
    }
    u32::try_from(numerator.saturating_mul(PARTS_PER_MILLION) / denominator)
        .unwrap_or(PARTS_PER_MILLION_U32)
}

fn pixel_offset(width: u32, x: u32, y: u32) -> Option<usize> {
    usize::try_from(
        u64::from(y)
            .checked_mul(u64::from(width))?
            .checked_add(u64::from(x))?,
    )
    .ok()?
    .checked_mul(4)
}

fn finding(
    code: SceneFindingCode,
    severity: SceneFindingSeverity,
    region_ids: Vec<String>,
    message: String,
) -> SceneInspectionFinding {
    SceneInspectionFinding {
        code,
        severity,
        region_ids,
        message,
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_immutable(
    path: &Path,
    bytes: &[u8],
    expected_hash: &str,
) -> Result<(), SceneInspectionNodeError> {
    if path.exists() {
        let actual_hash = sha256(&fs::read(path)?);
        if actual_hash == expected_hash {
            return Ok(());
        }
        return Err(SceneInspectionNodeError::ArtifactCollision {
            path: path.to_path_buf(),
        });
    }

    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temporary, bytes)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() && sha256(&fs::read(path)?) == expected_hash => {
            let _ = fs::remove_file(&temporary);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error.into())
        }
    }
}

/// Failure to validate, decode, inspect, or import a scene capture.
#[derive(Debug, Error)]
pub enum SceneInspectionNodeError {
    #[error("invalid scene inspection profile: {0}")]
    InvalidProfile(String),
    #[error("capture is not a PNG image")]
    NotPng,
    #[error("capture dimensions {width}x{height} exceed the decoder budget")]
    ImageTooLarge { width: u32, height: u32 },
    #[error("PNG decoder left an indexed pixel buffer after expansion")]
    UnexpectedIndexedPng,
    #[error("PNG decoder returned an unexpected pixel buffer")]
    UnexpectedPixelBuffer,
    #[error("imported artifact hash mismatch: expected {expected}, got {actual}")]
    ArtifactReadbackMismatch { expected: String, actual: String },
    #[error("imported artifact did not decode to the source image")]
    ArtifactDecodeMismatch,
    #[error("immutable artifact collision at {path}")]
    ArtifactCollision { path: PathBuf },
    #[error(transparent)]
    Png(#[from] png::DecodingError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(label: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("takegraph-scene-node-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(rgba).unwrap();
        }
        bytes
    }

    fn profile(width: u32, height: u32) -> SceneVisualCheckProfile {
        SceneVisualCheckProfile {
            expected_width: width,
            expected_height: height,
            black_luma_threshold: 8,
            black_pixel_ratio_ppm: 995_000,
            blank_channel_span_threshold: 2,
            safe_area: Some(PixelRect {
                x: 1,
                y: 1,
                width: width - 2,
                height: height - 2,
            }),
            regions: Vec::new(),
        }
    }

    #[test]
    fn scene_capture_digest_matches_cross_runtime_golden_and_sorts_frames() {
        assert_eq!(
            sha256(YMM4_SCENE_CAPTURE_DRIVER_ID.as_bytes()),
            YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST
        );
        let request = Ymm4SceneCaptureRequest::new(Ymm4SceneCaptureRequestInput {
            operation_id: Uuid::nil(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "fingerprint-a".into(),
            source_revision: 7,
            capture_profile_digest: "profile-a".into(),
            frames: vec![30, 10, 30, 20],
            alpha: true,
        });
        assert_eq!(request.frames, vec![10, 20, 30]);
        assert_eq!(
            request.request_digest,
            "931b8a38244a07c62e9181df5c739df70f2b8570ae3bdbb25a0817d85255c01c"
        );

        let changed = Ymm4SceneCaptureRequest::new(Ymm4SceneCaptureRequestInput {
            operation_id: Uuid::nil(),
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            expected_fingerprint: "fingerprint-a".into(),
            source_revision: 8,
            capture_profile_digest: "profile-a".into(),
            frames: vec![10, 20, 30],
            alpha: true,
        });
        assert_ne!(request.request_digest, changed.request_digest);
    }

    #[test]
    fn deserializes_direct_csharp_capture_receipt_shape() {
        let receipt: Ymm4SceneCaptureReceipt = serde_json::from_value(serde_json::json!({
            "operationId": "11111111-2222-4333-8444-555555555555",
            "requestDigest": "request-a",
            "projectId": "project-a",
            "sceneId": "scene-a",
            "sourceRevision": 7,
            "expectedFingerprint": "fingerprint-a",
            "captureProfileDigest": "profile-a",
            "status": "captured",
            "beforeFingerprint": "fingerprint-a",
            "afterFingerprint": "fingerprint-a",
            "frames": [{
                "requestedFrame": 10,
                "actualFrame": 10,
                "path": "C:\\staging\\frame.png",
                "sha256": "a".repeat(64),
                "width": 1920,
                "height": 1080,
                "mediaType": "image/png"
            }],
            "driver": YMM4_SCENE_CAPTURE_DRIVER_ID,
            "driverProfileDigest": YMM4_SCENE_CAPTURE_DRIVER_PROFILE_DIGEST,
            "transientStateRestored": true,
            "projectDirtyBefore": false,
            "projectDirtyAfter": false,
            "error": null
        }))
        .unwrap();

        assert_eq!(receipt.status, Ymm4SceneCaptureStatus::Captured);
        assert_eq!(receipt.frames[0].width, 1920);
        assert!(receipt.transient_state_restored);
    }

    #[test]
    fn imports_black_png_by_hash_and_reads_it_back() {
        let root = temporary_directory("black");
        let source = root.join("staging.png");
        let bytes = encode_rgba(4, 4, &[0, 0, 0, 255].repeat(16));
        fs::write(&source, &bytes).unwrap();
        let sample_id = Uuid::new_v4();

        let imported =
            import_png_capture(&source, &root, sample_id, 10, 10, &profile(4, 4)).unwrap();
        let replay = import_png_capture(&source, &root, sample_id, 10, 10, &profile(4, 4)).unwrap();

        assert_eq!(imported.sha256, sha256(&bytes));
        assert_eq!(imported.artifact_path, replay.artifact_path);
        assert_eq!(imported.width, 4);
        assert!(
            imported
                .inspection
                .findings
                .iter()
                .any(|finding| finding.code == SceneFindingCode::BlackFrame)
        );
        assert!(
            imported
                .inspection
                .findings
                .iter()
                .any(|finding| finding.code == SceneFindingCode::BlankFrame)
        );
        verify_imported_capture(&imported).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_caption_clipping_safe_area_and_portrait_overlap() {
        let root = temporary_directory("regions");
        let source = root.join("staging.png");
        let mut pixels = [20, 20, 20, 255].repeat(100);
        // Caption content reaches its left edge and spans x=3..=6, y=5..=7.
        for y in 5usize..8 {
            for x in 3usize..7 {
                let offset = (y * 10 + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        // Portrait content spans x=6..=8, y=3..=7 and overlaps caption at x=6.
        for y in 3usize..8 {
            for x in 6usize..9 {
                let offset = (y * 10 + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[200, 80, 80, 255]);
            }
        }
        fs::write(&source, encode_rgba(10, 10, &pixels)).unwrap();
        let mut checks = profile(10, 10);
        checks.safe_area = Some(PixelRect {
            x: 2,
            y: 2,
            width: 6,
            height: 6,
        });
        checks.regions = vec![
            VisualRegionExpectation {
                region_id: "caption-main".into(),
                kind: VisualRegionKind::Caption,
                bounds: PixelRect {
                    x: 3,
                    y: 5,
                    width: 4,
                    height: 3,
                },
                background: Rgba8 {
                    red: 20,
                    green: 20,
                    blue: 20,
                    alpha: 255,
                },
                color_tolerance: 5,
                min_foreground_ppm: 100_000,
                minimum_edge_clearance_px: 1,
            },
            VisualRegionExpectation {
                region_id: "portrait-main".into(),
                kind: VisualRegionKind::Portrait,
                bounds: PixelRect {
                    x: 5,
                    y: 2,
                    width: 5,
                    height: 7,
                },
                background: Rgba8 {
                    red: 20,
                    green: 20,
                    blue: 20,
                    alpha: 255,
                },
                color_tolerance: 5,
                min_foreground_ppm: 100_000,
                minimum_edge_clearance_px: 0,
            },
        ];

        let imported = import_png_capture(&source, &root, Uuid::new_v4(), 20, 20, &checks).unwrap();
        let codes: Vec<_> = imported
            .inspection
            .findings
            .iter()
            .map(|finding| finding.code)
            .collect();
        assert!(codes.contains(&SceneFindingCode::CaptionClipped));
        assert!(codes.contains(&SceneFindingCode::PortraitOutsideSafeArea));
        assert!(codes.contains(&SceneFindingCode::CaptionPortraitOverlap));
        assert!(!codes.contains(&SceneFindingCode::CaptionMissing));
        assert!(!codes.contains(&SceneFindingCode::PortraitMissing));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_missing_caption_and_portrait_regions() {
        let root = temporary_directory("missing-regions");
        let source = root.join("staging.png");
        let mut pixels = [20, 20, 20, 255].repeat(64);
        // Keep the whole frame non-uniform without adding foreground to either
        // expected region.
        pixels[0..4].copy_from_slice(&[40, 40, 40, 255]);
        fs::write(&source, encode_rgba(8, 8, &pixels)).unwrap();
        let mut checks = profile(8, 8);
        checks.regions = [
            ("caption", VisualRegionKind::Caption, 1),
            ("portrait", VisualRegionKind::Portrait, 4),
        ]
        .into_iter()
        .map(|(region_id, kind, x)| VisualRegionExpectation {
            region_id: region_id.into(),
            kind,
            bounds: PixelRect {
                x,
                y: 2,
                width: 3,
                height: 4,
            },
            background: Rgba8 {
                red: 20,
                green: 20,
                blue: 20,
                alpha: 255,
            },
            color_tolerance: 5,
            min_foreground_ppm: 10_000,
            minimum_edge_clearance_px: 0,
        })
        .collect();

        let imported = import_png_capture(&source, &root, Uuid::new_v4(), 1, 1, &checks).unwrap();
        let codes: Vec<_> = imported
            .inspection
            .findings
            .iter()
            .map(|finding| finding.code)
            .collect();
        assert!(codes.contains(&SceneFindingCode::CaptionMissing));
        assert!(codes.contains(&SceneFindingCode::PortraitMissing));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_dimension_mismatch_without_trusting_png_metadata_from_caller() {
        let root = temporary_directory("dimensions");
        let source = root.join("staging.png");
        let mut pixels = [20, 20, 20, 255].repeat(12);
        pixels[0] = 40;
        fs::write(&source, encode_rgba(4, 3, &pixels)).unwrap();

        let imported =
            import_png_capture(&source, &root, Uuid::new_v4(), 1, 1, &profile(4, 4)).unwrap();
        assert_eq!((imported.width, imported.height), (4, 3));
        assert!(
            imported
                .inspection
                .findings
                .iter()
                .any(|finding| finding.code == SceneFindingCode::DimensionsMismatch)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_non_png_and_invalid_regions() {
        let root = temporary_directory("invalid");
        let source = root.join("staging.png");
        fs::write(&source, b"not a png").unwrap();
        assert!(matches!(
            import_png_capture(&source, &root, Uuid::new_v4(), 0, 0, &profile(4, 4)),
            Err(SceneInspectionNodeError::NotPng)
        ));

        let mut invalid = profile(4, 4);
        invalid.regions.push(VisualRegionExpectation {
            region_id: "outside".into(),
            kind: VisualRegionKind::Caption,
            bounds: PixelRect {
                x: 3,
                y: 3,
                width: 2,
                height: 2,
            },
            background: Rgba8 {
                red: 0,
                green: 0,
                blue: 0,
                alpha: 255,
            },
            color_tolerance: 0,
            min_foreground_ppm: 1,
            minimum_edge_clearance_px: 0,
        });
        assert!(matches!(
            invalid.validate(),
            Err(SceneInspectionNodeError::InvalidProfile(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
