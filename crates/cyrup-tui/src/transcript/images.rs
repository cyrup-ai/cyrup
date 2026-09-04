use super::*;

/// One `image` content block of a tool result (`{type:"image", data, mimeType}`) — the wire mime type
/// plus the decoded raster, or `None` when the bytes were not a recognizable image. A block that
/// fails to decode still renders Pi's text stand-in ([`crate::image::image_fallback_text`]) so the
/// user is told an image came back.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultImage {
    /// The declared `mimeType` (`image/png`, …), or `image/unknown` when the block omitted it —
    /// Pi's own default in `getTextOutput` (render-utils.ts:53).
    pub mime_type: String,
    /// The decoded raster, downscaled to [`MAX_RASTER_PX`], or `None` if the base64/format could
    /// not be decoded.
    pub block: Option<ImageBlock>,
    /// The **source** pixel dimensions, before any downscale — what Pi's `imageFallback` reports
    /// (`getImageDimensions(img.data, img.mimeType)`, render-utils.ts:55-56).
    pub dimensions: Option<(u32, u32)>,
}

/// Pi's `terminal.imageWidthCells` default (settings-manager.ts:1060-1066) — the cell width an
/// inline tool-result image is clamped to (`maxWidthCells`, tool-execution.ts:348).
pub const DEFAULT_IMAGE_WIDTH_CELLS: u16 = 60;

/// Upper bound (px, either side) a tool-result image is downscaled to when it is decoded. A
/// half-block raster is at most a few dozen cells wide, so nothing above this is ever visible — and
/// the bound is what keeps the per-frame clone+resize of a screenshot-sized PNG off the render path.
const MAX_RASTER_PX: u32 = 1024;

/// Append Pi's `[Image: …]` text stand-in for each `image` content block (`imageFallback`,
/// terminal-image.ts:546-558, reached from `getTextOutput`, render-utils.ts:49-59) — used when
/// `showImages` is off or a block could not be decoded.
///
/// Divergence worth naming: Pi splices this into the tool's TEXT output, so a collapsed `read` (whose
/// `renderResult` returns `""` unless expanded) shows nothing at all. cyrup appends it to the block
/// unconditionally, matching the inline-raster case — which Pi also renders regardless of `expanded`
/// — so an image result is never silently invisible.
pub(super) fn push_image_fallbacks(run: &ToolRun, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    for img in &run.images {
        out.push(Line::styled(
            image_fallback_text(&img.mime_type, img.dimensions, None),
            theme.tool_output_style(),
        ));
    }
}

/// Rasterize each decoded `image` content block into half-block cell rows, each preceded by the
/// blank spacer Pi puts before every image component (`new Spacer(1)`, tool-execution.ts:342).
/// The raster is clamped to `width_cells` and to the content width. See
/// [`ImageBlock::halfblock_lines`] for why this is half-blocks rather than the negotiated
/// Kitty/iTerm2 protocol.
pub(super) fn image_raster_lines(
    run: &ToolRun,
    width: usize,
    width_cells: u16,
) -> Vec<Line<'static>> {
    let cols = width_cells.min(width.min(u16::MAX as usize) as u16).max(1);
    let mut out = Vec::new();
    for img in run.images.iter().filter_map(|i| i.block.as_ref()) {
        let rows = img.halfblock_lines(cols);
        if rows.is_empty() {
            continue;
        }
        out.push(Line::default());
        out.extend(rows);
    }
    out
}

/// Decode the `image` content blocks of a raw tool result (`{content:[{type:"image", data, mimeType}]}`)
/// into [`ResultImage`]s — Pi's `result.content.filter((c) => c.type === "image")`
/// (tool-execution.ts:331). A block whose base64 or pixel format cannot be decoded is kept with
/// `block: None` so its text stand-in still renders.
pub(super) fn decode_result_images(result: &Value) -> Vec<ResultImage> {
    use base64::Engine as _;
    let content = match result {
        Value::Object(o) => o.get("content"),
        Value::Array(_) => Some(result),
        _ => None,
    };
    let Some(Value::Array(items)) = content else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_object)
        .filter(|o| o.get("type").and_then(Value::as_str) == Some("image"))
        .map(|o| {
            let mime_type = o
                .get("mimeType")
                .or_else(|| o.get("mime_type"))
                .and_then(Value::as_str)
                .unwrap_or("image/unknown")
                .to_string();
            let decoded = o
                .get("data")
                .and_then(Value::as_str)
                .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                .and_then(|bytes| ImageBlock::decode(&bytes, mime_type.clone()));
            // Read the SOURCE dimensions (what Pi's `imageFallback` reports) before bounding the
            // raster the renderer will actually clone+resize each frame.
            let dimensions = decoded.as_ref().map(ImageBlock::dimensions);
            let block = decoded.map(|b| b.downscaled(MAX_RASTER_PX));
            ResultImage {
                mime_type,
                block,
                dimensions,
            }
        })
        .collect()
}
