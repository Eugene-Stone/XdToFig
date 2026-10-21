use std::{fs::File, io::Write, path::Path};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{
    error::{AppError, Result},
    parser::{XdArtboard, XdDocument, XdElement},
};

/// Write the public FigCraft archive format: a ZIP with document.json and
/// thumbnail.png. IDs and parent/child links are generated deterministically.
pub fn export(document: &XdDocument, output: &Path) -> Result<()> {
    let mut nodes = Map::new();
    let mut page_ids = Vec::new();
    let mut next_id = 1_u64;
    let mut referenced_images = Vec::<(String, Vec<u8>, u32, u32)>::new();

    for page in &document.pages {
        let page_id = next_id;
        next_id += 1;
        page_ids.push(page_id);
        let mut children = Vec::new();
        let page_origin = page
            .artboards
            .iter()
            .fold((f64::INFINITY, f64::INFINITY), |(x, y), artboard| {
                (x.min(artboard.x), y.min(artboard.y))
            });
        for artboard in &page.artboards {
            let id = build_artboard(
                artboard,
                (artboard.x - page_origin.0, artboard.y - page_origin.1),
                page_id,
                &mut next_id,
                &mut nodes,
                &document.resources,
                &mut referenced_images,
            );
            children.push(id);
        }
        nodes.insert(page_id.to_string(), json!({
            "id": page_id, "name": page.name, "type": "page", "background": color(255,255,255,255),
            "flows": [], "prototype": {"device":"None", "background":color(255,255,255,255), "rotation":"portrait"},
            "guides": [], "children": children, "width":0.0, "height":0.0
        }));
    }
    if page_ids.is_empty() {
        return Err(AppError::InvalidXd(
            "XD-документ не содержит страниц с артбордами".into(),
        ));
    }

    let current_page = page_ids[0];
    let image_index: Vec<Value> = referenced_images
        .iter()
        .map(|(hash, _, width, height)| json!({"hash":hash,"width":width,"height":height}))
        .collect();
    let value = json!({
        "format":"figcraft", "version":1, "imageIndex":image_index, "name":document.title,
        "pages":page_ids, "nodes":nodes, "next_id":next_id,
        "styles":[], "variables":{}, "current_page":current_page, "document_colors":[],
        "comments":[], "libraries":[]
    });
    let json_bytes = serde_json::to_vec(&value)?;
    let file = File::create(output).map_err(|source| AppError::Io {
        path: output.to_path_buf(),
        source,
    })?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("document.json", options)?;
    zip.write_all(&json_bytes).map_err(|source| AppError::Io {
        path: output.to_path_buf(),
        source,
    })?;
    zip.start_file("thumbnail.png", options)?;
    zip.write_all(EMPTY_THUMBNAIL_PNG)
        .map_err(|source| AppError::Io {
            path: output.to_path_buf(),
            source,
        })?;
    for (hash, bytes, _, _) in referenced_images {
        zip.start_file(format!("images/{hash}"), options)?;
        zip.write_all(&bytes).map_err(|source| AppError::Io {
            path: output.to_path_buf(),
            source,
        })?;
    }
    zip.finish()?;
    Ok(())
}

fn build_artboard(
    artboard: &XdArtboard,
    position: (f64, f64),
    parent: u64,
    next_id: &mut u64,
    nodes: &mut Map<String, Value>,
    resources: &[crate::parser::XdResource],
    images: &mut Vec<(String, Vec<u8>, u32, u32)>,
) -> u64 {
    let id = allocate(next_id);
    let mut children = Vec::new();
    for element in &artboard.children {
        children.push(build_element(
            element,
            id,
            (artboard.x, artboard.y),
            next_id,
            nodes,
            resources,
            images,
        ));
    }
    nodes.insert(
        id.to_string(),
        json!({
            "id":id, "name":artboard.name, "type":"frame", "parent":parent, "children":children,
            "width":positive(artboard.width), "height":positive(artboard.height),
            "fills":fills_from(&artboard.raw, &[], &mut Vec::new()).unwrap_or_else(|| json!([])), "clip":true,
            "transform":transform(&artboard.raw, position.0, position.1)
        }),
    );
    // XD's manifest bounds position the artboard on the pasteboard. Its children
    // are relative to the artboard, so keep that offset on the frame itself.
    id
}

fn build_element(
    element: &XdElement,
    parent: u64,
    source_origin: (f64, f64),
    next_id: &mut u64,
    nodes: &mut Map<String, Value>,
    resources: &[crate::parser::XdResource],
    images: &mut Vec<(String, Vec<u8>, u32, u32)>,
) -> u64 {
    let id = allocate(next_id);
    let kind = &element.kind;
    let mut child_ids = Vec::new();
    if matches!(kind.as_str(), "group" | "frame" | "artboard") {
        if let Some(children) = element
            .raw
            .get("normalized_children")
            .and_then(Value::as_array)
        {
            for child in children {
                let nested: XdElement = serde_json::from_value(child.clone())
                    .unwrap_or_else(|_| element_from_raw(child));
                child_ids.push(build_element(
                    &nested,
                    id,
                    (0.0, 0.0),
                    next_id,
                    nodes,
                    resources,
                    images,
                ));
            }
        } else {
            let children = element
                .raw
                .get("children")
                .and_then(Value::as_array)
                .or_else(|| {
                    element
                        .raw
                        .pointer("/group/children")
                        .and_then(Value::as_array)
                });
            if let Some(children) = children {
                for child in children {
                    let nested = element_from_raw(child);
                    child_ids.push(build_element(
                        &nested,
                        id,
                        (0.0, 0.0),
                        next_id,
                        nodes,
                        resources,
                        images,
                    ));
                }
            }
        }
    }
    let ty = match kind.as_str() {
        "text" => "text",
        "group" | "syncref" => "group",
        "rectangle" | "rect" => "rectangle",
        "ellipse" | "oval" => "ellipse",
        "line" => "line",
        "frame" | "artboard" => "frame",
        "polygon" => "polygon",
        _ => "vector",
    };
    let mut node = Map::new();
    node.insert("id".into(), json!(id));
    node.insert("name".into(), json!(element.name));
    node.insert("type".into(), json!(ty));
    node.insert("parent".into(), json!(parent));
    node.insert("children".into(), json!(child_ids));
    node.insert(
        "transform".into(),
        relative_transform(element, source_origin),
    );
    node.insert("width".into(), json!(positive(element.width)));
    node.insert("height".into(), json!(positive(element.height)));
    if let Some(fills) = fills_from(&element.raw, resources, images) {
        node.insert("fills".into(), fills);
    }
    if ty == "text" {
        let text = element
            .text
            .as_deref()
            .or_else(|| element.raw.get("characters").and_then(Value::as_str))
            .unwrap_or("");
        node.insert("characters".into(), json!(text));
        let (style, runs) = text_styles(&element.raw, text);
        node.insert("style".into(), style);
        if !runs.is_empty() {
            node.insert("runs".into(), Value::Array(runs));
        }
        node.insert("auto_resize".into(), json!("height"));
        node.insert("align_v".into(), json!("top"));
    }
    if let Some(corners) = element
        .raw
        .get("cornerRadius")
        .or_else(|| element.raw.get("cornerRadii"))
    {
        node.insert("corners".into(), corners.clone());
    }
    if ty == "vector" {
        let network = if let Some(path) = element
            .path_data
            .as_deref()
            .or_else(|| element.raw.get("path").and_then(Value::as_str))
        {
            svg_path_to_network(path, element.width, element.height)
        } else {
            // FigCraft requires a VectorNetwork even for unsupported/empty XD paths.
            json!({"vertices":[],"segments":[],"regions":[]})
        };
        node.insert("network".into(), network);
    }
    if let Some(stroke) = stroke_from(&element.raw) {
        node.insert("stroke".into(), stroke);
        node.insert("strokes".into(), json!([]));
    }
    nodes.insert(id.to_string(), Value::Object(node));
    id
}

fn element_from_raw(raw: &Value) -> XdElement {
    let actual = raw;
    let bounds = raw.get("shape").unwrap_or(raw);
    let kind = raw
        .pointer("/shape/type")
        .and_then(Value::as_str)
        .or_else(|| raw.get("type").and_then(Value::as_str))
        .unwrap_or("vector")
        .to_ascii_lowercase();
    let text = raw
        .pointer("/text/rawText")
        .and_then(Value::as_str)
        .or_else(|| raw.get("text").and_then(Value::as_str))
        .or_else(|| raw.pointer("/text/0/characters").and_then(Value::as_str))
        .map(str::to_owned);
    XdElement {
        id: raw.get("id").and_then(Value::as_str).map(str::to_owned),
        name: raw
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&kind)
            .to_owned(),
        kind: if text.is_some() || kind.contains("text") {
            "text".into()
        } else if kind.contains("group") {
            "group".into()
        } else if kind.contains("rect")
            || kind.contains("shape")
                && raw.pointer("/shape/type").and_then(Value::as_str) == Some("rect")
        {
            "rectangle".into()
        } else if kind == "shape" {
            raw.pointer("/shape/type")
                .and_then(Value::as_str)
                .unwrap_or("path")
                .to_owned()
        } else {
            kind
        },
        x: raw
            .pointer("/transform/tx")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| number(bounds, "x").unwrap_or(0.0)),
        y: raw
            .pointer("/transform/ty")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| number(bounds, "y").unwrap_or(0.0)),
        width: number(bounds, "width").unwrap_or(0.0),
        height: number(bounds, "height").unwrap_or(0.0),
        text,
        path_data: raw
            .pointer("/shape/path")
            .and_then(Value::as_str)
            .or_else(|| actual.get("path").and_then(Value::as_str))
            .map(str::to_owned),
        raw: raw.clone(),
    }
}

fn fills_from(
    raw: &Value,
    resources: &[crate::parser::XdResource],
    images: &mut Vec<(String, Vec<u8>, u32, u32)>,
) -> Option<Value> {
    let fill = raw
        .pointer("/style/fill")
        .or_else(|| raw.pointer("/fill"))?;
    if fill.get("type").and_then(Value::as_str) == Some("none") {
        return Some(json!([]));
    }
    if fill.get("type").and_then(Value::as_str) == Some("pattern") {
        let uid = fill
            .pointer("/pattern/meta/ux/uid")
            .and_then(Value::as_str)?;
        let resource = resources
            .iter()
            .find(|resource| resource.name.rsplit('/').next() == Some(uid))?;
        let hash = Sha256::digest(&resource.bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let width = fill
            .pointer("/pattern/width")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let height = fill
            .pointer("/pattern/height")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        if !images.iter().any(|(existing, _, _, _)| existing == &hash) {
            images.push((hash.clone(), resource.bytes.clone(), width, height));
        }
        return Some(
            json!([{"type":"image","image":hash,"scale_mode":"fill","opacity":1.0,"visible":true,"blend":"normal"}]),
        );
    }
    let c = fill
        .pointer("/color/value")
        .or_else(|| fill.get("color"))
        .or_else(|| fill.get("value"))?;
    let rgb = |key: &str| {
        c.get(key).and_then(Value::as_f64).unwrap_or(0.0)
            / if c.get(key).and_then(Value::as_f64).unwrap_or(0.0) > 1.0 {
                255.0
            } else {
                1.0
            }
    };
    let alpha = fill
        .pointer("/color/alpha")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| fill.get("opacity").and_then(Value::as_f64).unwrap_or(1.0));
    Some(
        json!([{"type":"solid","color":{"r":rgb("r"),"g":rgb("g"),"b":rgb("b"),"a":alpha},"opacity":1.0,"visible":true,"blend":"normal"}]),
    )
}

fn stroke_from(raw: &Value) -> Option<Value> {
    let stroke = raw.pointer("/style/stroke").or_else(|| raw.get("stroke"))?;
    let kind = stroke
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("solid");
    if kind == "none" {
        return None;
    }
    let color = stroke
        .pointer("/color/value")
        .or_else(|| stroke.get("color"))?;
    let component = |key: &str| {
        let value = color.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        if value > 1.0 {
            value / 255.0
        } else {
            value
        }
    };
    let alpha = stroke
        .pointer("/color/alpha")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let weight = stroke.get("width").and_then(Value::as_f64).unwrap_or(1.0);
    Some(
        json!({"weight":weight,"align":"center","sides":null,"cap_start":"none","cap_end":"none","join":"miter","miter_limit":4.0,"dashes":[],"paint":{"type":"solid","color":{"r":component("r"),"g":component("g"),"b":component("b"),"a":alpha},"opacity":1.0,"visible":true,"blend":"normal"}}),
    )
}

fn transform(raw: &Value, x: f64, y: f64) -> Value {
    let t = raw.get("transform");
    json!([
        t.and_then(|v| number(v, "a")).unwrap_or(1.0),
        t.and_then(|v| number(v, "b")).unwrap_or(0.0),
        t.and_then(|v| number(v, "c")).unwrap_or(0.0),
        t.and_then(|v| number(v, "d")).unwrap_or(1.0),
        t.and_then(|v| number(v, "tx")).unwrap_or(x),
        t.and_then(|v| number(v, "ty")).unwrap_or(y)
    ])
}

fn relative_transform(element: &XdElement, origin: (f64, f64)) -> Value {
    let mut value = transform(&element.raw, element.x, element.y);
    if let Some(components) = value.as_array_mut() {
        for (index, offset) in [(4, origin.0), (5, origin.1)] {
            let coordinate = components[index].as_f64().unwrap_or(0.0) - offset;
            components[index] = json!(coordinate);
        }
    }
    value
}

fn text_styles(raw: &Value, characters: &str) -> (Value, Vec<Value>) {
    let ranged = raw
        .pointer("/meta/ux/rangedStyles")
        .and_then(Value::as_array);
    let first_range = ranged.and_then(|ranges| ranges.first());
    let base = text_style(raw, first_range);
    let Some(ranges) = ranged else {
        return (base, Vec::new());
    };

    let char_count = characters.chars().count();
    let mut utf16_cursor = 0usize;
    let mut runs = Vec::new();
    for range in ranges {
        let length = range.get("length").and_then(Value::as_u64).unwrap_or(0) as usize;
        let start = utf16_to_char_index(characters, utf16_cursor).min(char_count);
        utf16_cursor = utf16_cursor.saturating_add(length);
        let end = utf16_to_char_index(characters, utf16_cursor).min(char_count);
        if start >= end {
            continue;
        }
        let style = text_style(raw, Some(range));
        if style != base {
            runs.push(json!({"start":start,"end":end,"style":style}));
        }
    }
    (base, runs)
}

fn utf16_to_char_index(text: &str, offset: usize) -> usize {
    let mut units = 0usize;
    let mut chars = 0usize;
    for ch in text.chars() {
        if units >= offset {
            break;
        }
        units += ch.len_utf16();
        chars += 1;
    }
    chars
}

fn text_style(raw: &Value, range: Option<&Value>) -> Value {
    let font = raw.pointer("/style/font");
    let family = range
        .and_then(|style| style.get("fontFamily"))
        .and_then(Value::as_str)
        .or_else(|| {
            font.and_then(|font| font.get("family"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Inter");
    let font_style = range
        .and_then(|style| style.get("fontStyle"))
        .and_then(Value::as_str)
        .or_else(|| {
            font.and_then(|font| font.get("style"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Regular");
    let size = range
        .and_then(|style| style.get("fontSize"))
        .and_then(Value::as_f64)
        .or_else(|| {
            font.and_then(|font| font.get("size"))
                .and_then(Value::as_f64)
        })
        .or_else(|| raw.pointer("/text/style/fontSize").and_then(Value::as_f64))
        .unwrap_or(14.0);
    let mut style = json!({
        "font_family":family,
        "font_style":font_style,
        "font_size":size,
    });
    if let Some(character_spacing) = range
        .and_then(|style| style.get("charSpacing"))
        .and_then(Value::as_f64)
    {
        style["letter_spacing"] = json!({"unit":"pixels","value":character_spacing});
    }
    let decoration = if range
        .and_then(|style| style.get("underline"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        Some("underline")
    } else if range
        .and_then(|style| style.get("strikethrough"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        Some("strikethrough")
    } else {
        None
    };
    if let Some(decoration) = decoration {
        style["decoration"] = json!(decoration);
    }
    if let Some(fill) = range.and_then(|style| style.get("fill")) {
        if let Some(paint) = packed_color_paint(fill.get("value")) {
            style["fills"] = json!([paint]);
        }
    }
    style
}

fn packed_color_paint(value: Option<&Value>) -> Option<Value> {
    let argb = value?.as_u64()? as u32;
    let channel = |shift: u32| f64::from((argb >> shift) & 0xff) / 255.0;
    Some(json!({
        "type":"solid",
        "color":{"r":channel(16),"g":channel(8),"b":channel(0),"a":channel(24)},
        "opacity":1.0,"visible":true,"blend":"normal"
    }))
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}
fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
fn allocate(next: &mut u64) -> u64 {
    let id = *next;
    *next += 1;
    id
}
fn color(r: u8, g: u8, b: u8, a: u8) -> Value {
    json!({"r":r as f64/255.0,"g":g as f64/255.0,"b":b as f64/255.0,"a":a as f64/255.0})
}

// Convert SVG path commands to FigCraft's editable vertex/segment network.
fn svg_path_to_network(path: &str, width: f64, height: f64) -> Value {
    let mut vertices = Vec::new();
    let mut segments = Vec::new();
    let mut regions = Vec::new();
    let tokens = path_tokens(path);
    let mut i = 0usize;
    let mut command = ' ';
    let mut current = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut contour_vertices = Vec::<usize>::new();
    let mut contour_segments = Vec::<usize>::new();
    let mut contour_closed = false;
    let mut has_cubic = false;
    let scale_x = if width.abs() > 0.001 { width } else { 1.0 };
    let scale_y = if height.abs() > 0.001 { height } else { 1.0 };
    while i < tokens.len() {
        if let PathToken::Command(c) = tokens[i] {
            command = c;
            i += 1;
        }
        if command == 'Z' || command == 'z' {
            contour_closed = true;
            if current != start && !contour_vertices.is_empty() {
                let si = segments.len();
                segments.push(json!({"a":*contour_vertices.last().unwrap(),"b":contour_vertices[0],"ta":[0.0,0.0],"tb":[0.0,0.0]}));
                contour_segments.push(si);
            }
            current = start;
            finish_path_contour(
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                contour_closed,
                &mut regions,
            );
            contour_vertices.clear();
            contour_segments.clear();
            contour_closed = false;
            command = ' ';
            continue;
        }
        if command == ' ' {
            break;
        }
        let relative = command.is_ascii_lowercase();
        let upper = command.to_ascii_uppercase();
        if upper == 'M' || upper == 'L' || upper == 'T' {
            let Some(x) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y) = take_number(&tokens, &mut i) else {
                break;
            };
            let p = if relative {
                (current.0 + x, current.1 + y)
            } else {
                (x, y)
            };
            if upper == 'M' {
                finish_path_contour(
                    &mut vertices,
                    &mut segments,
                    &mut contour_vertices,
                    &mut contour_segments,
                    contour_closed,
                    &mut regions,
                );
                contour_vertices.clear();
                contour_segments.clear();
                contour_closed = false;
                start = p;
                command = if relative { 'l' } else { 'L' };
            }
            add_path_vertex(
                p,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                None,
            );
        } else if upper == 'H' || upper == 'V' {
            let Some(n) = take_number(&tokens, &mut i) else {
                break;
            };
            let p = if upper == 'H' {
                (if relative { current.0 + n } else { n }, current.1)
            } else {
                (current.0, if relative { current.1 + n } else { n })
            };
            add_path_vertex(
                p,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                None,
            );
        } else if upper == 'C' {
            let Some(x1) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y1) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(x2) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y2) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(x) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y) = take_number(&tokens, &mut i) else {
                break;
            };
            let base = current;
            let cv = if relative {
                (base.0 + x, base.1 + y)
            } else {
                (x, y)
            };
            let c1 = if relative {
                (base.0 + x1, base.1 + y1)
            } else {
                (x1, y1)
            };
            let c2 = if relative {
                (base.0 + x2, base.1 + y2)
            } else {
                (x2, y2)
            };
            add_path_vertex(
                cv,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                Some((
                    (c1.0 - base.0) * scale_x,
                    (c1.1 - base.1) * scale_y,
                    (c2.0 - cv.0) * scale_x,
                    (c2.1 - cv.1) * scale_y,
                )),
            );
            has_cubic = true;
        } else if upper == 'Q' {
            let Some(x1) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y1) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(x) = take_number(&tokens, &mut i) else {
                break;
            };
            let Some(y) = take_number(&tokens, &mut i) else {
                break;
            };
            let base = current;
            let end = if relative {
                (base.0 + x, base.1 + y)
            } else {
                (x, y)
            };
            let q = if relative {
                (base.0 + x1, base.1 + y1)
            } else {
                (x1, y1)
            };
            let c1 = (
                base.0 + 2.0 / 3.0 * (q.0 - base.0),
                base.1 + 2.0 / 3.0 * (q.1 - base.1),
            );
            let c2 = (
                end.0 + 2.0 / 3.0 * (q.0 - end.0),
                end.1 + 2.0 / 3.0 * (q.1 - end.1),
            );
            add_path_vertex(
                end,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                Some((
                    (c1.0 - base.0) * scale_x,
                    (c1.1 - base.1) * scale_y,
                    (c2.0 - end.0) * scale_x,
                    (c2.1 - end.1) * scale_y,
                )),
            );
            has_cubic = true;
        } else if upper == 'S' || upper == 'T' {
            // Smooth curve commands need the previous control point; approximate with their endpoint.
            let count = if upper == 'S' { 6 } else { 4 };
            let mut vals = Vec::new();
            for _ in 0..count {
                if let Some(n) = take_number(&tokens, &mut i) {
                    vals.push(n)
                } else {
                    break;
                }
            }
            if vals.len() != count {
                break;
            }
            let (x, y) = if upper == 'S' {
                (vals[4], vals[5])
            } else {
                (vals[2], vals[3])
            };
            let p = if relative {
                (current.0 + x, current.1 + y)
            } else {
                (x, y)
            };
            add_path_vertex(
                p,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                None,
            );
            has_cubic = true;
        } else if upper == 'A' {
            let mut vals = Vec::new();
            for _ in 0..7 {
                if let Some(n) = take_number(&tokens, &mut i) {
                    vals.push(n)
                } else {
                    break;
                }
            }
            if vals.len() != 7 {
                break;
            }
            let p = if relative {
                (current.0 + vals[5], current.1 + vals[6])
            } else {
                (vals[5], vals[6])
            };
            add_path_vertex(
                p,
                &mut current,
                scale_x,
                scale_y,
                &mut vertices,
                &mut segments,
                &mut contour_vertices,
                &mut contour_segments,
                None,
            );
            has_cubic = true;
        } else {
            break;
        }
        if vertices.len() > 20000 {
            break;
        }
    }
    finish_path_contour(
        &mut vertices,
        &mut segments,
        &mut contour_vertices,
        &mut contour_segments,
        contour_closed,
        &mut regions,
    );
    let _ = has_cubic;
    json!({"vertices":vertices,"segments":segments,"regions":regions})
}

fn finish_path_contour(
    vertices: &mut Vec<Value>,
    segments: &mut Vec<Value>,
    contour_vertices: &mut Vec<usize>,
    contour_segments: &mut Vec<usize>,
    explicitly_closed: bool,
    regions: &mut Vec<Value>,
) {
    let (Some(&first), Some(&mut_last)) = (contour_vertices.first(), contour_vertices.last())
    else {
        return;
    };
    if contour_segments.is_empty() {
        return;
    }

    let point = |index: usize| -> Option<(f64, f64)> {
        let vertex = vertices.get(index)?;
        Some((vertex.get("x")?.as_f64()?, vertex.get("y")?.as_f64()?))
    };
    let same_endpoint = point(first)
        .zip(point(mut_last))
        .is_some_and(|((x1, y1), (x2, y2))| (x1 - x2).hypot(y1 - y2) <= 1e-6);
    let mut last = mut_last;

    // Some XD paths close by ending at their first point without a `Z` command.
    // Merge that duplicate endpoint so FigCraft can treat the contour as a fill.
    if same_endpoint && last != first {
        for segment in segments.iter_mut() {
            if segment["a"].as_u64() == Some(last as u64) {
                segment["a"] = json!(first);
            }
            if segment["b"].as_u64() == Some(last as u64) {
                segment["b"] = json!(first);
            }
        }
        if last + 1 == vertices.len() {
            vertices.pop();
        }
        contour_vertices.pop();
        last = first;
    }

    let already_closed = segments
        .get(*contour_segments.last().unwrap_or(&usize::MAX))
        .and_then(|segment| segment["b"].as_u64())
        == Some(first as u64);
    if explicitly_closed && !already_closed {
        let index = segments.len();
        segments.push(json!({"a":last,"b":first,"ta":[0.0,0.0],"tb":[0.0,0.0]}));
        contour_segments.push(index);
    }

    if (explicitly_closed || same_endpoint) && contour_vertices.len() > 2 {
        // FigCraft regions refer to segment indices grouped by closed contour.
        regions.push(json!({"loops":[contour_segments],"winding":"nonZero"}));
    }
}

#[derive(Clone, Copy)]
enum PathToken {
    Command(char),
    Number(f64),
}
fn path_tokens(path: &str) -> Vec<PathToken> {
    let chars: Vec<char> = path.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_whitespace() || c == ',' {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            out.push(PathToken::Command(c));
            i += 1;
            continue;
        }
        let start = i;
        if c == '+' || c == '-' {
            i += 1;
        }
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
            i += 1;
            if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                i += 1;
            }
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        if let Ok(n) = chars[start..i].iter().collect::<String>().parse() {
            out.push(PathToken::Number(n));
        } else {
            i += 1;
        }
    }
    out
}
fn take_number(tokens: &[PathToken], i: &mut usize) -> Option<f64> {
    match tokens.get(*i) {
        Some(PathToken::Number(n)) => {
            *i += 1;
            Some(*n)
        }
        _ => None,
    }
}
fn add_path_vertex(
    p: (f64, f64),
    current: &mut (f64, f64),
    sx: f64,
    sy: f64,
    vertices: &mut Vec<Value>,
    segments: &mut Vec<Value>,
    contour_vertices: &mut Vec<usize>,
    contour_segments: &mut Vec<usize>,
    handles: Option<(f64, f64, f64, f64)>,
) {
    let id = vertices.len();
    vertices.push(json!({"x":p.0*sx,"y":p.1*sy,"radius":0.0,"mirror":"none","join":"miter"}));
    if let Some(&previous) = contour_vertices.last() {
        let (ta, tb) = handles
            .map(|(x1, y1, x2, y2)| (json!([x1, y1]), json!([x2, y2])))
            .unwrap_or((json!([0.0, 0.0]), json!([0.0, 0.0])));
        let segment_id = segments.len();
        segments.push(json!({"a":previous,"b":id,"ta":ta,"tb":tb}));
        contour_segments.push(segment_id);
    }
    contour_vertices.push(id);
    *current = p;
}

// A tiny transparent 1×1 PNG; FigCraft uses this as the document preview until re-rendered.
const EMPTY_THUMBNAIL_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1, 13, 10,
    45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];
