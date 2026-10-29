use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

const SURFACE_MAGIC: &[u8] = &[0x09, 0x4c, 0x59, 0x52, 0x54, 0x52, 0x41, 0x43, 0x45];
const WINDOW_MAGIC: &[u8] = &[0x09, 0x57, 0x49, 0x4e, 0x54, 0x52, 0x41, 0x43, 0x45];
const TRANSACTION_MAGIC: &[u8] = &[0x09, 0x54, 0x4e, 0x58, 0x54, 0x52, 0x41, 0x43, 0x45];
const TRANSITION_MAGIC: &[u8] = &[0x09, 0x54, 0x52, 0x4e, 0x54, 0x52, 0x41, 0x43, 0x45];
const SHELL_TRANSITION_MAGIC: &[u8] = &[0x09, 0x57, 0x4d, 0x53, 0x54, 0x52, 0x41, 0x43, 0x45];
const MAX_TRACE_ARCHIVE_ENTRIES: usize = 4096;
const MAX_TRACE_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TRACE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceDocument {
    pub source_name: String,
    pub surface_flinger: Option<ParsedHierarchy>,
    pub window_manager: Option<ParsedHierarchy>,
    pub transactions: Vec<TransactionEntry>,
    pub transitions: Vec<TransitionEntry>,
    pub timeline: Vec<TimelinePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedHierarchy {
    pub title: String,
    pub snapshots: Vec<HierarchySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchySnapshot {
    #[serde(serialize_with = "serialize_i64_as_string")]
    pub timestamp_ns: i64,
    pub timestamp: String,
    pub tree: TraceNode,
    pub rects: Vec<RectLayer>,
    pub properties_by_id: BTreeMap<String, Vec<PropertyGroup>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub visible: bool,
    #[serde(default)]
    pub children: Vec<TraceNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RectLayer {
    pub id: String,
    pub name: String,
    pub depth: f64,
    pub group_id: i64,
    pub color: String,
    pub visible: bool,
    pub is_display: bool,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyGroup {
    pub title: String,
    pub rows: Vec<PropertyRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyRow {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub changed: bool,
    #[serde(default)]
    pub default_value: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionEntry {
    pub id: String,
    pub time: String,
    #[serde(serialize_with = "serialize_i64_as_string")]
    pub timestamp_ns: i64,
    pub transaction_id: String,
    pub vsync_id: String,
    pub pid: String,
    pub uid: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub layer_or_display_id: String,
    pub what: String,
    pub properties: Vec<PropertyGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionEntry {
    pub id: i32,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub send_time: String,
    pub dispatch_time: String,
    pub duration: String,
    pub status: String,
    #[serde(serialize_with = "serialize_i64_as_string")]
    pub timestamp_ns: i64,
    pub properties: Vec<PropertyGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePoint {
    pub index: usize,
    pub label: String,
    pub timestamp: String,
    pub traces: BTreeMap<String, bool>,
}

fn serialize_i64_as_string<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[derive(Debug, Clone)]
enum FieldValue {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone)]
struct Field {
    number: u32,
    value: FieldValue,
}

#[derive(Debug, Clone)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Rect {
    fn width(&self) -> f64 {
        (self.right - self.left).max(0.0)
    }

    fn height(&self) -> f64 {
        (self.bottom - self.top).max(0.0)
    }
}

#[derive(Debug, Clone)]
struct SfLayer {
    id: i64,
    name: String,
    kind: String,
    visible: bool,
    depth: f64,
    parent: Option<i64>,
    children: Vec<i64>,
    rect: Option<Rect>,
    has_bounds: bool,
    has_visible_region: bool,
    position: Option<(f64, f64)>,
    requested_position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
    crop: Option<Rect>,
    requested_crop: Option<Rect>,
    destination_frame: Option<Rect>,
    active_buffer: Option<(u64, u64)>,
    has_active_buffer_content: bool,
    frame_number: Option<u64>,
    corner_radius: Option<f64>,
    requested_corner_radius: Option<f64>,
    shadow_radius: Option<f64>,
    background_blur_radius: Option<i64>,
    relative_parent: Option<i64>,
    color: Option<String>,
    requested_color: Option<String>,
    has_input_channel: bool,
    dataspace: Option<String>,
    pixel_format: Option<String>,
    flags: u64,
    layer_stack: u64,
}

#[derive(Debug, Clone)]
struct DisplayInfo {
    id: i64,
    name: String,
    layer_stack: u64,
    width: f64,
    height: f64,
}

pub fn parse_trace_bytes(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let lower_name = source_name.to_ascii_lowercase();
    if lower_name.contains("windowmanager") || lower_name.contains("window manager") {
        return parse_window_manager(bytes, source_name);
    }
    if lower_name.contains("surfaceflinger") || lower_name.contains("surface flinger") {
        return parse_surface_flinger(bytes, source_name);
    }
    if lower_name.contains("transaction") {
        return parse_transactions(bytes, source_name);
    }
    if lower_name.contains("transition") {
        return parse_transitions(bytes, source_name);
    }
    parse_named_bytes(bytes, source_name).ok_or_else(|| {
        format!(
            "Unsupported trace format: {}. Expected a legacy WinScope .pb or .zip archive.",
            source_name
        )
    })?
}

pub fn parse_trace_archive(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Cannot open trace archive: {error}"))?;
    if archive.len() > MAX_TRACE_ARCHIVE_ENTRIES {
        return Err(format!(
            "The trace archive contains too many trace archive entries (maximum {MAX_TRACE_ARCHIVE_ENTRIES})."
        ));
    }
    let mut document = empty_document(source_name);
    let mut recognized = false;
    let mut total_uncompressed_bytes = 0u64;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Cannot read archive entry {index}: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        validate_trace_entry_size(entry.size())?;
        total_uncompressed_bytes = total_uncompressed_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "The trace archive uncompressed size is invalid.".to_string())?;
        if total_uncompressed_bytes > MAX_TRACE_ARCHIVE_BYTES {
            return Err(format!(
                "The trace archive is too large after decompression (maximum {} MB).",
                MAX_TRACE_ARCHIVE_BYTES / 1024 / 1024
            ));
        }
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| format!("Cannot read archive entry {name}: {error}"))?;
        if let Some(parsed) = parse_named_bytes(&data, &name) {
            merge_documents(&mut document, parsed?);
            recognized = true;
        }
    }

    if !recognized {
        return Err("The archive contains no supported WinScope trace files.".to_string());
    }
    rebuild_timeline(&mut document);
    Ok(document)
}

fn parse_named_bytes(bytes: &[u8], source_name: &str) -> Option<Result<TraceDocument, String>> {
    if bytes.starts_with(SURFACE_MAGIC) {
        return Some(parse_surface_flinger(bytes, source_name));
    }
    if bytes.starts_with(WINDOW_MAGIC) {
        return Some(parse_window_manager(bytes, source_name));
    }
    if bytes.starts_with(TRANSACTION_MAGIC) {
        return Some(parse_transactions(bytes, source_name));
    }
    if bytes.starts_with(TRANSITION_MAGIC) || bytes.starts_with(SHELL_TRANSITION_MAGIC) {
        return Some(parse_transitions(bytes, source_name));
    }
    None
}

fn empty_document(source_name: &str) -> TraceDocument {
    TraceDocument {
        source_name: source_name.to_string(),
        surface_flinger: None,
        window_manager: None,
        transactions: Vec::new(),
        transitions: Vec::new(),
        timeline: Vec::new(),
    }
}

fn merge_documents(target: &mut TraceDocument, source: TraceDocument) {
    let mut source = source;
    if target.surface_flinger.is_none() {
        target.surface_flinger = source.surface_flinger.take();
    }
    if target.window_manager.is_none() {
        target.window_manager = source.window_manager.take();
    }
    target.transactions.append(&mut source.transactions);
    target.transitions.append(&mut source.transitions);
    target.source_name = if target.source_name.is_empty() {
        source.source_name
    } else {
        target.source_name.clone()
    };
}

fn validate_trace_entry_size(size: u64) -> Result<(), String> {
    if size > MAX_TRACE_ENTRY_BYTES {
        return Err(format!(
            "The trace archive contains a single trace entry larger than {} MB.",
            MAX_TRACE_ENTRY_BYTES / 1024 / 1024
        ));
    }
    Ok(())
}

fn parse_surface_flinger(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let fields = parse_fields(bytes)?;
    let offset = field_i64(&fields, 3).unwrap_or(0);
    let entries = field_bytes(&fields, 2);
    if entries.is_empty() {
        return Err(format!(
            "SurfaceFlinger trace contains no entries: {source_name}"
        ));
    }

    let mut snapshots = Vec::with_capacity(entries.len());
    for (index, entry_bytes) in entries.into_iter().enumerate() {
        let entry = parse_fields(entry_bytes)?;
        let excludes_composition_state = field_bool(&entry, 5).unwrap_or(false);
        let elapsed = field_i64(&entry, 1).unwrap_or(0);
        let timestamp_ns = elapsed.saturating_add(offset);
        let timestamp = format_timestamp(timestamp_ns);
        let mut layers = Vec::new();
        if let Some(layers_bytes) = field_bytes(&entry, 3).first() {
            let layers_fields = parse_fields(layers_bytes)?;
            for layer_bytes in field_bytes(&layers_fields, 1) {
                layers.push(parse_sf_layer(layer_bytes)?);
            }
        }

        let displays = field_bytes(&entry, 7)
            .into_iter()
            .filter_map(|display| parse_display(display).ok().flatten())
            .collect::<Vec<_>>();
        let display_name = displays
            .first()
            .map(|display| display.name.clone())
            .unwrap_or_else(|| "Built-in Screen".to_string());
        snapshots.push(make_surface_snapshot(
            index,
            timestamp_ns,
            timestamp,
            layers,
            displays,
            excludes_composition_state,
            display_name,
        ));
    }

    let mut document = empty_document(source_name);
    document.surface_flinger = Some(ParsedHierarchy {
        title: "LAYERS".to_string(),
        snapshots,
    });
    rebuild_timeline(&mut document);
    Ok(document)
}

fn parse_sf_layer(bytes: &[u8]) -> Result<SfLayer, String> {
    let fields = parse_fields(bytes)?;
    let id = field_i64(&fields, 1).unwrap_or(0);
    let name = field_string(&fields, 2).unwrap_or_else(|| format!("Layer {id}"));
    let kind = field_string(&fields, 5).unwrap_or_else(|| "Layer".to_string());
    let flags = field_u64(&fields, 22).unwrap_or(0);
    let visible = flags & 0x01 == 0;
    let children = field_packed_i64(&fields, 3);
    let parent = field_i64(&fields, 25).filter(|value| *value != 0);
    let position = field_bytes(&fields, 11)
        .first()
        .and_then(|value| parse_position(value).ok().flatten());
    let requested_position = field_bytes(&fields, 12)
        .first()
        .and_then(|value| parse_position(value).ok().flatten());
    let size = field_bytes(&fields, 13)
        .first()
        .and_then(|value| parse_size(value).ok().flatten());
    let crop = field_bytes(&fields, 14)
        .first()
        .and_then(|value| parse_int_rect(value).ok().flatten());
    let requested_crop = field_bytes(&fields, 15)
        .first()
        .and_then(|value| parse_int_rect(value).ok().flatten());
    let destination_frame = field_bytes(&fields, 57)
        .first()
        .and_then(|value| parse_int_rect(value).ok().flatten());
    let active_buffer = field_bytes(&fields, 27)
        .first()
        .and_then(|value| parse_fields(value).ok())
        .map(|buffer| {
            (
                field_u64(&buffer, 1).unwrap_or(0),
                field_u64(&buffer, 2).unwrap_or(0),
            )
        });
    let has_active_buffer_content = field_bytes(&fields, 27)
        .first()
        .and_then(|value| parse_active_buffer_content(value).ok())
        .unwrap_or(false);
    let bounds_rect = field_bytes(&fields, 45)
        .first()
        .and_then(|value| parse_float_rect(value).ok().flatten());
    let has_visible_region = field_bytes(&fields, 7)
        .first()
        .and_then(|value| parse_region_has_area(value).ok())
        .unwrap_or(false);
    let rect = bounds_rect
        .clone()
        .or_else(|| {
            field_bytes(&fields, 46)
                .first()
                .and_then(|value| parse_float_rect(value).ok().flatten())
        })
        .or_else(|| {
            field_bytes(&fields, 57)
                .first()
                .and_then(|value| parse_int_rect(value).ok().flatten())
        })
        .or_else(|| match (position, size) {
            (Some((left, top)), Some((width, height))) => Some(Rect {
                left,
                top,
                right: left + width,
                bottom: top + height,
            }),
            _ => None,
        });

    Ok(SfLayer {
        id,
        name,
        kind,
        visible,
        depth: field_i64(&fields, 10).unwrap_or(0) as f64,
        parent,
        children,
        rect,
        has_bounds: bounds_rect.is_some(),
        has_visible_region,
        position,
        requested_position,
        size,
        crop,
        requested_crop,
        destination_frame,
        active_buffer,
        has_active_buffer_content,
        frame_number: field_u64(&fields, 37),
        corner_radius: field_f32(&fields, 41).map(f64::from),
        requested_corner_radius: field_f32(&fields, 56).map(f64::from),
        shadow_radius: field_f32(&fields, 49).map(f64::from),
        background_blur_radius: field_i64(&fields, 52),
        relative_parent: field_i64(&fields, 26).filter(|value| *value != -1),
        color: field_bytes(&fields, 20)
            .first()
            .and_then(|value| parse_color(value).ok().flatten()),
        requested_color: field_bytes(&fields, 21)
            .first()
            .and_then(|value| parse_color(value).ok().flatten()),
        has_input_channel: !field_bytes(&fields, 47).is_empty(),
        dataspace: field_string(&fields, 18),
        pixel_format: field_string(&fields, 19),
        flags,
        layer_stack: field_u64(&fields, 9).unwrap_or(0),
    })
}

fn parse_display(bytes: &[u8]) -> Result<Option<DisplayInfo>, String> {
    let fields = parse_fields(bytes)?;
    let size = field_bytes(&fields, 4)
        .first()
        .and_then(|value| parse_size(value).ok().flatten());
    let (width, height) = size.unwrap_or((0.0, 0.0));
    Ok(Some(DisplayInfo {
        id: field_u64(&fields, 1).unwrap_or(0) as i64,
        name: field_string(&fields, 2).unwrap_or_else(|| "Built-in Screen".to_string()),
        layer_stack: field_u64(&fields, 3).unwrap_or(0),
        width,
        height,
    }))
}

fn parse_region_has_area(bytes: &[u8]) -> Result<bool, String> {
    let fields = parse_fields(bytes)?;
    Ok(field_bytes(&fields, 2).iter().any(|rect| {
        parse_int_rect(rect)
            .ok()
            .flatten()
            .map(|rect| rect.width() > 0.0 && rect.height() > 0.0)
            .unwrap_or(false)
    }))
}

fn parse_active_buffer_content(bytes: &[u8]) -> Result<bool, String> {
    let fields = parse_fields(bytes)?;
    Ok(field_u64(&fields, 1).unwrap_or(0) > 0
        || field_u64(&fields, 2).unwrap_or(0) > 0
        || field_u64(&fields, 3).unwrap_or(0) > 0
        || field_u64(&fields, 4).unwrap_or(0) > 0)
}

fn make_surface_snapshot(
    index: usize,
    timestamp_ns: i64,
    timestamp: String,
    layers: Vec<SfLayer>,
    displays: Vec<DisplayInfo>,
    excludes_composition_state: bool,
    display_name: String,
) -> HierarchySnapshot {
    let by_id: HashMap<i64, SfLayer> = layers
        .iter()
        .cloned()
        .map(|layer| (layer.id, layer))
        .collect();
    let mut rect_order = layers.iter().enumerate().collect::<Vec<_>>();
    rect_order.sort_by(|(left_index, left), (right_index, right)| {
        left.depth
            .partial_cmp(&right.depth)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left_index.cmp(right_index))
    });
    let rect_depth_by_id = rect_order
        .into_iter()
        .enumerate()
        .map(|(depth, (_, layer))| (layer.id, depth as f64))
        .collect::<HashMap<_, _>>();
    let mut children_by_parent: HashMap<i64, Vec<i64>> = HashMap::new();
    for layer in &layers {
        if let Some(parent) = layer.parent {
            children_by_parent.entry(parent).or_default().push(layer.id);
        }
    }
    for layer in &layers {
        if !layer.children.is_empty() {
            children_by_parent.insert(layer.id, layer.children.clone());
        }
    }

    let mut root_ids: Vec<i64> = layers
        .iter()
        .filter(|layer| {
            layer.parent.is_none() || !by_id.contains_key(&layer.parent.unwrap_or_default())
        })
        .map(|layer| layer.id)
        .collect();
    if root_ids.is_empty() {
        root_ids = layers.iter().map(|layer| layer.id).collect();
    }
    root_ids.sort_by_key(|id| by_id.get(id).map(|layer| layer.depth as i64).unwrap_or(0));

    let mut visited = HashSet::new();
    let tree_children = root_ids
        .into_iter()
        .filter_map(|id| make_sf_node(id, &by_id, &children_by_parent, &mut visited))
        .collect::<Vec<_>>();
    let root_id = format!("sf-root-{index}");
    let mut properties_by_id = BTreeMap::new();
    properties_by_id.insert(
        root_id.clone(),
        vec![PropertyGroup {
            title: "Snapshot".to_string(),
            rows: vec![
                property("timestamp", &timestamp),
                property("display", &display_name),
            ],
        }],
    );
    for layer in &layers {
        properties_by_id.insert(format!("sf-layer-{}", layer.id), sf_properties(layer));
    }

    let display_count = displays.len() as f64;
    let mut rects = displays
        .iter()
        .enumerate()
        .map(|(index, display)| RectLayer {
            id: format!("sf-display-{}", display.id),
            name: display.name.clone(),
            depth: index as f64,
            group_id: display.layer_stack as i64,
            color: "#b9c6d4".to_string(),
            visible: false,
            is_display: true,
            left: 0.0,
            top: 0.0,
            width: display.width,
            height: display.height,
        })
        .collect::<Vec<_>>();
    rects.extend(
        layers
            .iter()
            .filter(|layer| {
                layer.has_bounds
                    && layer.visible
                    && (excludes_composition_state || layer.has_visible_region)
                    && (layer.has_active_buffer_content
                        || layer.color.is_some()
                        || layer.shadow_radius.unwrap_or(0.0) > 0.0)
            })
            .map(|layer| RectLayer {
                id: format!("sf-layer-{}", layer.id),
                name: layer.name.clone(),
                depth: rect_depth_by_id.get(&layer.id).copied().unwrap_or_default() + display_count,
                group_id: layer.layer_stack as i64,
                color: if layer.visible { "#6f9fd7" } else { "#b9c6d4" }.to_string(),
                visible: layer.visible,
                is_display: false,
                left: layer.rect.as_ref().map(|rect| rect.left).unwrap_or(0.0),
                top: layer.rect.as_ref().map(|rect| rect.top).unwrap_or(0.0),
                width: layer.rect.as_ref().map(Rect::width).unwrap_or(1.0),
                height: layer.rect.as_ref().map(Rect::height).unwrap_or(1.0),
            }),
    );

    HierarchySnapshot {
        timestamp_ns,
        timestamp: timestamp.clone(),
        tree: TraceNode {
            id: root_id,
            label: format!("LayerTraceEntry - {timestamp}"),
            kind: "root".to_string(),
            visible: true,
            children: tree_children,
        },
        rects,
        properties_by_id,
    }
}

fn make_sf_node(
    id: i64,
    by_id: &HashMap<i64, SfLayer>,
    children_by_parent: &HashMap<i64, Vec<i64>>,
    visited: &mut HashSet<i64>,
) -> Option<TraceNode> {
    if !visited.insert(id) {
        return None;
    }
    let layer = by_id.get(&id)?;
    let children = children_by_parent
        .get(&id)
        .into_iter()
        .flatten()
        .filter_map(|child| make_sf_node(*child, by_id, children_by_parent, visited))
        .collect();
    Some(TraceNode {
        id: format!("sf-layer-{id}"),
        label: format!("{} - {}", id, layer.name),
        kind: if layer.kind.is_empty() {
            "item"
        } else {
            &layer.kind
        }
        .to_string(),
        visible: layer.visible,
        children,
    })
}

fn parse_window_manager(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let fields = parse_fields(bytes)?;
    let offset = field_i64(&fields, 3).unwrap_or(0);
    let entries = field_bytes(&fields, 2);
    if entries.is_empty() {
        return Err(format!(
            "WindowManager trace contains no entries: {source_name}"
        ));
    }

    let mut snapshots = Vec::with_capacity(entries.len());
    for (index, entry_bytes) in entries.into_iter().enumerate() {
        let entry = parse_fields(entry_bytes)?;
        let elapsed = field_i64(&entry, 1).unwrap_or(0);
        let timestamp_ns = elapsed.saturating_add(offset);
        let timestamp = format_timestamp(timestamp_ns);
        let root_children: Vec<Vec<u8>> = field_bytes(&entry, 3)
            .first()
            .map(|service| service.to_vec())
            .and_then(|service| parse_fields(&service).ok())
            .and_then(|service_fields| {
                field_bytes(&service_fields, 2)
                    .first()
                    .map(|value| value.to_vec())
            })
            .and_then(|root| parse_fields(&root).ok())
            .and_then(|root_fields| {
                field_bytes(&root_fields, 1)
                    .first()
                    .map(|value| value.to_vec())
            })
            .and_then(|container| parse_fields(&container).ok())
            .map(|container_fields| {
                field_bytes(&container_fields, 5)
                    .into_iter()
                    .map(Vec::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut properties_by_id = BTreeMap::new();
        let mut rects = Vec::new();
        let children = root_children
            .iter()
            .enumerate()
            .map(|(child_index, child)| {
                make_wm_node(
                    child,
                    &format!("wm-{index}-{child_index}"),
                    0.0,
                    &mut rects,
                    &mut properties_by_id,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let root_id = format!("wm-root-{index}");
        properties_by_id.insert(
            root_id.clone(),
            vec![PropertyGroup {
                title: "Snapshot".to_string(),
                rows: vec![property("timestamp", &timestamp)],
            }],
        );
        snapshots.push(HierarchySnapshot {
            timestamp_ns,
            timestamp,
            tree: TraceNode {
                id: root_id,
                label: format!("WindowManagerState - {}", format_timestamp(timestamp_ns)),
                kind: "root".to_string(),
                visible: true,
                children,
            },
            rects,
            properties_by_id,
        });
    }

    let mut document = empty_document(source_name);
    document.window_manager = Some(ParsedHierarchy {
        title: "WINDOWS".to_string(),
        snapshots,
    });
    rebuild_timeline(&mut document);
    Ok(document)
}

fn make_wm_node(
    bytes: &[u8],
    id: &str,
    depth: f64,
    rects: &mut Vec<RectLayer>,
    properties_by_id: &mut BTreeMap<String, Vec<PropertyGroup>>,
) -> Result<TraceNode, String> {
    let fields = parse_fields(bytes)?;
    let (variant, payload) = if let Some(value) = field_bytes(&fields, 3).first() {
        ("DisplayContent", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 4).first() {
        ("DisplayArea", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 5).first() {
        ("Task", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 6).first() {
        ("Activity", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 7).first() {
        ("WindowToken", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 8).first() {
        ("WindowState", (*value).to_vec())
    } else if let Some(value) = field_bytes(&fields, 2).first() {
        ("WindowContainer", (*value).to_vec())
    } else {
        ("WindowContainer", Vec::new())
    };
    let payload_fields = parse_fields(&payload)?;
    let (label, visible, rect, children_bytes, group_id) =
        wm_node_details(variant, &payload, &payload_fields);
    let node_id = id.to_string();
    let children = children_bytes
        .iter()
        .enumerate()
        .map(|(index, child)| {
            make_wm_node(
                child,
                &format!("{id}-{index}"),
                depth + 1.0,
                rects,
                properties_by_id,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let properties = wm_properties(variant, &label, visible, rect.as_ref(), children.len());
    properties_by_id.insert(node_id.clone(), properties.clone());
    if matches!(variant, "DisplayContent" | "WindowState") {
        if let Some(rect) = &rect {
            let is_display = variant == "DisplayContent";
            rects.push(RectLayer {
                id: node_id.clone(),
                name: if is_display {
                    label.replacen("DisplayContent", "Display", 1)
                } else {
                    label.clone()
                },
                depth,
                group_id,
                color: if visible && !is_display {
                    "#6f9fd7"
                } else {
                    "#b9c6d4"
                }
                .to_string(),
                visible,
                is_display,
                left: rect.left,
                top: rect.top,
                width: rect.width(),
                height: rect.height(),
            });
        }
    }
    Ok(TraceNode {
        id: node_id,
        label,
        kind: variant.to_ascii_lowercase(),
        visible,
        children,
    })
}

fn wm_node_details(
    variant: &str,
    payload: &[u8],
    fields: &[Field],
) -> (String, bool, Option<Rect>, Vec<Vec<u8>>, i64) {
    let container: Option<Vec<u8>> = match variant {
        "DisplayContent" => field_bytes(fields, 21)
            .first()
            .and_then(|area| parse_fields(area).ok())
            .and_then(|area_fields| {
                field_bytes(&area_fields, 1)
                    .first()
                    .map(|value| value.to_vec())
            }),
        "DisplayArea" => field_bytes(fields, 1).first().map(|value| value.to_vec()),
        "Task" => field_bytes(fields, 31)
            .first()
            .and_then(|fragment| parse_fields(fragment).ok())
            .and_then(|fragment_fields| {
                field_bytes(&fragment_fields, 1)
                    .first()
                    .map(|value| value.to_vec())
            })
            .or_else(|| field_bytes(fields, 1).first().map(|value| value.to_vec())),
        "Activity" | "WindowToken" | "WindowState" => {
            field_bytes(fields, 1).first().map(|value| value.to_vec())
        }
        _ => Some(payload.to_vec()),
    };
    let children = container
        .as_deref()
        .and_then(|value| parse_fields(value).ok())
        .map(|container_fields| {
            field_bytes(&container_fields, 5)
                .into_iter()
                .map(Vec::from)
                .collect()
        })
        .unwrap_or_default();
    let visible = container
        .as_deref()
        .and_then(|value| parse_fields(value).ok())
        .and_then(|container_fields| field_bool(&container_fields, 3))
        .or_else(|| match variant {
            "Activity" => field_bool(fields, 24),
            "WindowState" => field_bool(fields, 38),
            _ => None,
        })
        .unwrap_or(true);
    let label = match variant {
        "DisplayContent" => {
            let name = field_bytes(fields, 10)
                .first()
                .and_then(|info| parse_fields(info).ok())
                .and_then(|info_fields| field_string(&info_fields, 5))
                .unwrap_or_else(|| "Built-in Screen".to_string());
            format!("DisplayContent - {name}")
        }
        "DisplayArea" => format!(
            "DisplayArea - {}",
            field_string(fields, 2).unwrap_or_default()
        ),
        "Task" => format!("Task - {}", field_i64(fields, 2).unwrap_or_default()),
        "Activity" => format!("Activity - {}", field_string(fields, 1).unwrap_or_default()),
        "WindowToken" => format!(
            "WindowToken - {:x}",
            field_u64(fields, 2).unwrap_or_default()
        ),
        "WindowState" => {
            let title = field_bytes(fields, 2)
                .first()
                .and_then(|identifier| parse_fields(identifier).ok())
                .and_then(|identifier_fields| field_string(&identifier_fields, 3))
                .or_else(|| {
                    container
                        .as_deref()
                        .and_then(|value| parse_fields(value).ok())
                        .and_then(|container_fields| {
                            field_bytes(&container_fields, 6)
                                .first()
                                .map(|value| value.to_vec())
                        })
                        .and_then(|identifier| parse_fields(&identifier).ok())
                        .and_then(|identifier_fields| field_string(&identifier_fields, 3))
                })
                .unwrap_or_default();
            format!("WindowState - {title}")
        }
        _ => "WindowContainer".to_string(),
    };
    let rect = match variant {
        "DisplayContent" => field_bytes(fields, 10)
            .first()
            .and_then(|info| parse_fields(info).ok())
            .and_then(|info_fields| {
                let width = field_i64(&info_fields, 1)? as f64;
                let height = field_i64(&info_fields, 2)? as f64;
                Some(Rect {
                    left: 0.0,
                    top: 0.0,
                    right: width,
                    bottom: height,
                })
            }),
        "Task" => field_bytes(fields, 5)
            .first()
            .and_then(|value| parse_int_rect(value).ok().flatten()),
        "WindowState" => field_bytes(fields, 7)
            .first()
            .and_then(|value| parse_int_rect(value).ok().flatten())
            .or_else(|| {
                field_bytes(fields, 41)
                    .first()
                    .and_then(|frames| parse_fields(frames).ok())
                    .and_then(|frame_fields| {
                        field_bytes(&frame_fields, 5)
                            .first()
                            .map(|value| value.to_vec())
                    })
                    .and_then(|value| parse_int_rect(&value).ok().flatten())
            }),
        _ => None,
    };
    let group_id = match variant {
        "DisplayContent" => field_i64(fields, 2),
        "WindowState" => field_i64(fields, 3),
        _ => None,
    }
    .unwrap_or(0);
    (label, visible, rect, children, group_id)
}

fn wm_properties(
    variant: &str,
    label: &str,
    visible: bool,
    rect: Option<&Rect>,
    child_count: usize,
) -> Vec<PropertyGroup> {
    let geometry = rect
        .map(|rect| {
            format!(
                "({}, {}) - ({}, {})",
                rect.left, rect.top, rect.right, rect.bottom
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    vec![
        PropertyGroup {
            title: "Visibility".to_string(),
            rows: vec![property("visible", &visible.to_string())],
        },
        PropertyGroup {
            title: "Geometry".to_string(),
            rows: vec![property("bounds", &geometry)],
        },
        PropertyGroup {
            title: "Hierarchy".to_string(),
            rows: vec![
                property("type", variant),
                property("children", &child_count.to_string()),
            ],
        },
        PropertyGroup {
            title: "Proto".to_string(),
            rows: vec![property("name", label)],
        },
    ]
}

fn parse_transactions(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let fields = parse_fields(bytes)?;
    let offset = field_i64(&fields, 3).unwrap_or(0);
    let mut entries = Vec::new();
    for (entry_index, entry_bytes) in field_bytes(&fields, 2).into_iter().enumerate() {
        let entry = parse_fields(entry_bytes)?;
        let timestamp_ns = field_i64(&entry, 1).unwrap_or(0).saturating_add(offset);
        let time = format_timestamp(timestamp_ns);
        let vsync_id = field_i64(&entry, 2).unwrap_or(0).to_string();
        for (state_index, state_bytes) in field_bytes(&entry, 3).into_iter().enumerate() {
            let state = parse_fields(state_bytes)?;
            let transaction_id = field_u64(&state, 6)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let pid = field_i64(&state, 1)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let uid = field_i64(&state, 2)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let layer_states = field_bytes(&state, 7);
            if layer_states.is_empty() {
                entries.push(transaction_entry(
                    &format!("transaction-{entry_index}-{state_index}"),
                    timestamp_ns,
                    &time,
                    &transaction_id,
                    &vsync_id,
                    &pid,
                    &uid,
                    "TRANSACTION",
                    "N/A",
                    "",
                ));
            } else {
                for (layer_index, layer_bytes) in layer_states.into_iter().enumerate() {
                    let layer = parse_fields(layer_bytes)?;
                    let layer_id = field_u64(&layer, 1)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "N/A".to_string());
                    let what = field_u64(&layer, 2)
                        .map(|value| format!("0x{value:x}"))
                        .unwrap_or_else(|| "eLayerChanged".to_string());
                    entries.push(transaction_entry(
                        &format!("transaction-{entry_index}-{state_index}-{layer_index}"),
                        timestamp_ns,
                        &time,
                        &transaction_id,
                        &vsync_id,
                        &pid,
                        &uid,
                        "LAYER_CHANGED",
                        &layer_id,
                        &what,
                    ));
                }
            }
        }
    }
    let mut document = empty_document(source_name);
    document.transactions = entries;
    rebuild_timeline(&mut document);
    Ok(document)
}

fn transaction_entry(
    id: &str,
    timestamp_ns: i64,
    time: &str,
    transaction_id: &str,
    vsync_id: &str,
    pid: &str,
    uid: &str,
    entry_type: &str,
    layer_id: &str,
    what: &str,
) -> TransactionEntry {
    TransactionEntry {
        id: id.to_string(),
        time: time.to_string(),
        timestamp_ns,
        transaction_id: transaction_id.to_string(),
        vsync_id: vsync_id.to_string(),
        pid: pid.to_string(),
        uid: uid.to_string(),
        entry_type: entry_type.to_string(),
        layer_or_display_id: layer_id.to_string(),
        what: what.to_string(),
        properties: vec![PropertyGroup {
            title: "Transaction".to_string(),
            rows: vec![
                property("transactionId", transaction_id),
                property("what", what),
                property("layerId", layer_id),
            ],
        }],
    }
}

fn parse_transitions(bytes: &[u8], source_name: &str) -> Result<TraceDocument, String> {
    let fields = parse_fields(bytes)?;
    let offset_field = if bytes.starts_with(SHELL_TRANSITION_MAGIC) {
        4
    } else {
        3
    };
    let offset = field_i64(&fields, offset_field).unwrap_or(0);
    let mut entries = Vec::new();
    for transition_bytes in field_bytes(&fields, 2) {
        let transition = parse_fields(transition_bytes)?;
        let id = field_i64(&transition, 1).unwrap_or(0) as i32;
        let (send_field, dispatch_field, finish_field, abort_field, type_field) =
            if bytes.starts_with(SHELL_TRANSITION_MAGIC) {
                (3, 4, 9, 7, 13)
            } else {
                (5, 0, 6, 10, 7)
            };
        let send_ns = field_i64(&transition, send_field)
            .unwrap_or(0)
            .saturating_add(offset);
        let dispatch_ns = if dispatch_field == 0 {
            0
        } else {
            field_i64(&transition, dispatch_field)
                .unwrap_or(0)
                .saturating_add(offset)
        };
        let finish_ns = field_i64(&transition, finish_field)
            .unwrap_or(0)
            .saturating_add(offset);
        let status = if field_i64(&transition, abort_field).unwrap_or(0) != 0 {
            "ABORTED"
        } else {
            "PLAYED"
        };
        let duration = if finish_ns > send_ns {
            format_duration(finish_ns - send_ns)
        } else {
            "n/a".to_string()
        };
        let entry_type = transition_type(field_i64(&transition, type_field).unwrap_or(0));
        entries.push(TransitionEntry {
            id,
            entry_type: entry_type.clone(),
            send_time: if send_ns == 0 {
                "n/a".to_string()
            } else {
                format_timestamp(send_ns)
            },
            dispatch_time: if dispatch_ns == 0 {
                "n/a".to_string()
            } else {
                format_timestamp(dispatch_ns)
            },
            duration,
            status: status.to_string(),
            timestamp_ns: if send_ns == 0 { finish_ns } else { send_ns },
            properties: vec![PropertyGroup {
                title: "Transition".to_string(),
                rows: vec![
                    property("id", &id.to_string()),
                    property("type", &entry_type),
                    property("status", status),
                ],
            }],
        });
    }
    let mut document = empty_document(source_name);
    document.transitions = entries;
    rebuild_timeline(&mut document);
    Ok(document)
}

fn transition_type(value: i64) -> String {
    match value {
        0 => "NONE".to_string(),
        1 => "OPEN".to_string(),
        2 => "CLOSE".to_string(),
        3 => "TO_FRONT".to_string(),
        4 => "TO_BACK".to_string(),
        5 => "CHANGE".to_string(),
        6 => "SLEEP".to_string(),
        other => format!("TYPE_{other}"),
    }
}

fn rebuild_timeline(document: &mut TraceDocument) {
    let mut points: BTreeMap<i64, BTreeMap<String, bool>> = BTreeMap::new();
    if let Some(surface) = &document.surface_flinger {
        for snapshot in &surface.snapshots {
            points
                .entry(snapshot.timestamp_ns)
                .or_default()
                .insert("surfaceFlinger".to_string(), true);
        }
    }
    if let Some(window) = &document.window_manager {
        for snapshot in &window.snapshots {
            points
                .entry(snapshot.timestamp_ns)
                .or_default()
                .insert("windowManager".to_string(), true);
        }
    }
    for entry in &document.transactions {
        points
            .entry(entry.timestamp_ns)
            .or_default()
            .insert("transactions".to_string(), true);
    }
    for entry in &document.transitions {
        points
            .entry(entry.timestamp_ns)
            .or_default()
            .insert("transitions".to_string(), true);
    }
    document.timeline = points
        .into_iter()
        .enumerate()
        .map(|(index, (timestamp_ns, traces))| TimelinePoint {
            index,
            label: format_timestamp_label(timestamp_ns),
            timestamp: format!("{timestamp_ns} ns"),
            traces,
        })
        .collect();
}

fn property(label: &str, value: &str) -> PropertyRow {
    PropertyRow {
        label: label.to_string(),
        value: value.to_string(),
        changed: false,
        default_value: false,
    }
}

fn sf_properties(layer: &SfLayer) -> Vec<PropertyGroup> {
    let bounds = format_rect_value(layer.rect.as_ref());
    let crop = format_rect_value(layer.crop.as_ref());
    let requested_crop = format_rect_value(layer.requested_crop.as_ref());
    let destination_frame = format_rect_value(layer.destination_frame.as_ref());
    let position = format_pair(layer.position);
    let requested_position = format_pair(layer.requested_position);
    let size = format_pair(layer.size);
    let buffer_size = layer
        .active_buffer
        .map(|(width, height)| format!("{width} x {height}"))
        .or_else(|| {
            if size != "n/a" {
                Some(size.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "n/a".to_string());
    let relative_parent = layer
        .relative_parent
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    vec![
        PropertyGroup {
            title: "Visibility".to_string(),
            rows: vec![
                property("visible", &layer.visible.to_string()),
                property("flags", &format!("0x{:x}", layer.flags)),
                property("summary", if layer.visible { "Visible" } else { "Hidden" }),
            ],
        },
        PropertyGroup {
            title: "Geometry".to_string(),
            rows: vec![
                property("transform", "n/a"),
                property("crop", &crop),
                property("final bounds", &bounds),
                property("position", &position),
                property("requested position", &requested_position),
                property("requested crop", &requested_crop),
            ],
        },
        PropertyGroup {
            title: "Buffer".to_string(),
            rows: vec![
                property("size", &buffer_size),
                property(
                    "Frame Number",
                    &layer
                        .frame_number
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
                property("transform", "n/a"),
                property("destination frame", &destination_frame),
            ],
        },
        PropertyGroup {
            title: "Hierarchy".to_string(),
            rows: vec![
                property("z-order", &layer.depth.to_string()),
                property(
                    "parent",
                    &layer
                        .parent
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                ),
                property("relative parent", &relative_parent),
                property("layerStack", &layer.layer_stack.to_string()),
            ],
        },
        PropertyGroup {
            title: "Effects".to_string(),
            rows: vec![
                property("color", layer.color.as_deref().unwrap_or("n/a")),
                property(
                    "requested color",
                    layer.requested_color.as_deref().unwrap_or("n/a"),
                ),
                property(
                    "corner radius",
                    &layer
                        .corner_radius
                        .map(|value| format!("{value} px"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
                property(
                    "requested corner radius",
                    &layer
                        .requested_corner_radius
                        .map(|value| format!("{value} px"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
                property(
                    "shadow",
                    &layer
                        .shadow_radius
                        .map(|value| format!("{value} px"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
                property(
                    "blur",
                    &layer
                        .background_blur_radius
                        .map(|value| format!("{value} px"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
            ],
        },
        PropertyGroup {
            title: "Input".to_string(),
            rows: vec![
                property(
                    "input channel",
                    if layer.has_input_channel {
                        "set"
                    } else {
                        "not set"
                    },
                ),
                property("input config", "n/a"),
            ],
        },
        PropertyGroup {
            title: "Proto".to_string(),
            rows: vec![
                property("name", &layer.name),
                property("id", &layer.id.to_string()),
                property("type", &layer.kind),
                property("dataspace", layer.dataspace.as_deref().unwrap_or("n/a")),
                property(
                    "pixel format",
                    layer.pixel_format.as_deref().unwrap_or("n/a"),
                ),
            ],
        },
    ]
}

fn format_rect_value(rect: Option<&Rect>) -> String {
    rect.map(|rect| {
        format!(
            "({}, {}) - ({}, {})",
            rect.left, rect.top, rect.right, rect.bottom
        )
    })
    .unwrap_or_else(|| "n/a".to_string())
}

fn format_pair(pair: Option<(f64, f64)>) -> String {
    pair.map(|(x, y)| format!("({}, {})", x, y))
        .unwrap_or_else(|| "n/a".to_string())
}

fn parse_fields(bytes: &[u8]) -> Result<Vec<Field>, String> {
    let mut fields = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let (key, next) = read_varint(bytes, cursor)?;
        cursor = next;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x07) as u8;
        if number == 0 {
            return Err("Invalid protobuf field number 0".to_string());
        }
        let value = match wire_type {
            0 => {
                let (value, next) = read_varint(bytes, cursor)?;
                cursor = next;
                FieldValue::Varint(value)
            }
            1 => {
                if cursor + 8 > bytes.len() {
                    return Err("Truncated protobuf fixed64 field".to_string());
                }
                let value = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
                cursor += 8;
                FieldValue::Fixed64(value)
            }
            2 => {
                let (length, next) = read_varint(bytes, cursor)?;
                cursor = next;
                let end = cursor
                    .checked_add(length as usize)
                    .ok_or_else(|| "Invalid protobuf length".to_string())?;
                if end > bytes.len() {
                    return Err("Truncated protobuf bytes field".to_string());
                }
                let value = bytes[cursor..end].to_vec();
                cursor = end;
                FieldValue::Bytes(value)
            }
            5 => {
                if cursor + 4 > bytes.len() {
                    return Err("Truncated protobuf fixed32 field".to_string());
                }
                let value = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
                cursor += 4;
                FieldValue::Fixed32(value)
            }
            3 | 4 => return Err("Grouped protobuf fields are not supported".to_string()),
            _ => return Err(format!("Unsupported protobuf wire type {wire_type}")),
        };
        fields.push(Field { number, value });
    }
    Ok(fields)
}

fn read_varint(bytes: &[u8], mut cursor: usize) -> Result<(u64, usize), String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(cursor)
            .ok_or_else(|| "Truncated protobuf varint".to_string())?;
        cursor += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, cursor));
        }
    }
    Err("Invalid protobuf varint".to_string())
}

fn field_value<'a>(fields: &'a [Field], number: u32) -> Option<&'a FieldValue> {
    fields
        .iter()
        .find(|field| field.number == number)
        .map(|field| &field.value)
}

fn field_bytes<'a>(fields: &'a [Field], number: u32) -> Vec<&'a [u8]> {
    fields
        .iter()
        .filter(|field| field.number == number)
        .filter_map(|field| match &field.value {
            FieldValue::Bytes(value) => Some(value.as_slice()),
            _ => None,
        })
        .collect()
}

fn field_u64(fields: &[Field], number: u32) -> Option<u64> {
    match field_value(fields, number)? {
        FieldValue::Varint(value) | FieldValue::Fixed64(value) => Some(*value),
        FieldValue::Fixed32(value) => Some(*value as u64),
        FieldValue::Bytes(_) => None,
    }
}

fn field_i64(fields: &[Field], number: u32) -> Option<i64> {
    field_u64(fields, number).map(|value| value as i64)
}

fn field_bool(fields: &[Field], number: u32) -> Option<bool> {
    field_u64(fields, number).map(|value| value != 0)
}

fn field_string(fields: &[Field], number: u32) -> Option<String> {
    match field_value(fields, number)? {
        FieldValue::Bytes(value) => String::from_utf8(value.clone()).ok(),
        _ => None,
    }
}

fn field_packed_i64(fields: &[Field], number: u32) -> Vec<i64> {
    let mut values = Vec::new();
    for field in fields.iter().filter(|field| field.number == number) {
        match &field.value {
            FieldValue::Varint(value) => values.push(*value as i64),
            FieldValue::Bytes(bytes) => {
                let mut cursor = 0;
                while cursor < bytes.len() {
                    if let Ok((value, next)) = read_varint(bytes, cursor) {
                        values.push(value as i64);
                        cursor = next;
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    values
}

fn field_f32(fields: &[Field], number: u32) -> Option<f32> {
    match field_value(fields, number)? {
        FieldValue::Fixed32(value) => Some(f32::from_bits(*value)),
        _ => None,
    }
}

fn parse_float_rect(bytes: &[u8]) -> Result<Option<Rect>, String> {
    let fields = parse_fields(bytes)?;
    Ok(Some(Rect {
        left: field_f32(&fields, 1).unwrap_or(0.0) as f64,
        top: field_f32(&fields, 2).unwrap_or(0.0) as f64,
        right: field_f32(&fields, 3).unwrap_or(0.0) as f64,
        bottom: field_f32(&fields, 4).unwrap_or(0.0) as f64,
    }))
}

fn parse_int_rect(bytes: &[u8]) -> Result<Option<Rect>, String> {
    let fields = parse_fields(bytes)?;
    Ok(Some(Rect {
        left: field_i64(&fields, 1).unwrap_or(0) as f64,
        top: field_i64(&fields, 2).unwrap_or(0) as f64,
        right: field_i64(&fields, 3).unwrap_or(0) as f64,
        bottom: field_i64(&fields, 4).unwrap_or(0) as f64,
    }))
}

fn parse_position(bytes: &[u8]) -> Result<Option<(f64, f64)>, String> {
    let fields = parse_fields(bytes)?;
    Ok(Some((
        field_f32(&fields, 1).unwrap_or(0.0) as f64,
        field_f32(&fields, 2).unwrap_or(0.0) as f64,
    )))
}

fn parse_size(bytes: &[u8]) -> Result<Option<(f64, f64)>, String> {
    let fields = parse_fields(bytes)?;
    Ok(Some((
        field_i64(&fields, 1).unwrap_or(0) as f64,
        field_i64(&fields, 2).unwrap_or(0) as f64,
    )))
}

fn parse_color(bytes: &[u8]) -> Result<Option<String>, String> {
    let fields = parse_fields(bytes)?;
    if field_f32(&fields, 1).is_none()
        && field_f32(&fields, 2).is_none()
        && field_f32(&fields, 3).is_none()
    {
        return Ok(None);
    }
    Ok(Some(format!(
        "({}, {}, {}, {})",
        field_f32(&fields, 1).unwrap_or(0.0),
        field_f32(&fields, 2).unwrap_or(0.0),
        field_f32(&fields, 3).unwrap_or(0.0),
        field_f32(&fields, 4).unwrap_or(1.0),
    )))
}

fn format_timestamp(timestamp_ns: i64) -> String {
    let seconds = timestamp_ns.div_euclid(1_000_000_000);
    let nanos = timestamp_ns.rem_euclid(1_000_000_000) as u32;
    Utc.timestamp_opt(seconds, nanos)
        .single()
        .map(|value| value.format("%Y-%m-%dT%H:%M:%S%.9f").to_string())
        .unwrap_or_else(|| format!("{timestamp_ns} ns"))
}

fn format_timestamp_label(timestamp_ns: i64) -> String {
    let seconds = timestamp_ns.div_euclid(1_000_000_000);
    let nanos = timestamp_ns.rem_euclid(1_000_000_000) as u32;
    Utc.timestamp_opt(seconds, nanos)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S%.9f").to_string())
        .unwrap_or_else(|| format!("{timestamp_ns} ns"))
}

fn format_duration(nanos: i64) -> String {
    let millis = nanos / 1_000_000;
    let remainder = nanos % 1_000_000;
    format!("{millis}ms{remainder}ns")
}

#[cfg(test)]
mod tests {
    use super::{
        parse_fields, parse_trace_archive, validate_trace_entry_size, MAX_TRACE_ENTRY_BYTES,
    };

    #[test]
    fn rejects_trace_archive_entries_over_the_single_entry_limit() {
        let error = validate_trace_entry_size(MAX_TRACE_ENTRY_BYTES + 1)
            .expect_err("an oversized trace entry must be rejected before reading it");
        assert!(error.contains("single trace entry"));
    }

    #[test]
    fn rejects_trace_archives_with_too_many_members() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for index in 0..=4096 {
            writer
                .start_file(format!("entry-{index}.pb"), options)
                .expect("test archive member should be created");
        }
        let archive = writer
            .finish()
            .expect("test archive should finish")
            .into_inner();
        let error = parse_trace_archive(&archive, "too-many-entries.zip")
            .expect_err("an archive with too many members must be rejected");
        assert!(error.contains("too many trace archive entries"));
    }

    #[test]
    fn decodes_fixed64_and_length_delimited_fields() {
        let bytes = [0x09, 1, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x01, 0x41];
        let fields = parse_fields(&bytes).expect("wire data should parse");
        assert_eq!(fields.len(), 2);
    }
}
