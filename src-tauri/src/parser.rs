use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::ZipArchive;

use crate::error::{AppError, Result};

/// Нормализованное представление документа XD.
/// Неизвестные поля исходного JSON сохраняются в `raw`, чтобы расширять маппер без потери данных.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdDocument {
    pub title: String,
    pub manifest: Value,
    pub pages: Vec<XdPage>,
    pub artwork: Vec<XdArtworkFile>,
    pub resources: Vec<XdResource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdResource {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdPage {
    pub name: String,
    pub artboards: Vec<XdArtboard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdArtboard {
    pub id: Option<String>,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub children: Vec<XdElement>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdElement {
    pub id: Option<String>,
    pub name: String,
    /// Нормализованный тип, например `rectangle`, `text`, `path`, `group`.
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub text: Option<String>,
    pub path_data: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XdArtworkFile {
    pub path: String,
    pub json: Value,
}

/// Читает XD ZIP целиком в RAM, не создавая временных файлов.
pub fn parse_xd_file(path: &Path) -> Result<XdDocument> {
    let file = File::open(path).map_err(|source| AppError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;

    let mut manifest: Option<Value> = None;
    let mut artwork_json = BTreeMap::<String, Value>::new();
    let mut resources = Vec::<XdResource>::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name()?.replace('\\', "/");

        // Guard against ZIP path traversal even though entries are only read, not extracted.
        if name.split('/').any(|part| part == "..") {
            return Err(AppError::InvalidXd(format!(
                "небезопасный путь внутри архива: {name}"
            )));
        }

        let basename = name.rsplit('/').next().unwrap_or_default();
        let is_manifest = basename == "manifest" || basename == "manifest.json";
        // Adobe XD stores its graphics JSON in `.agc` files.
        let is_artwork_json = name.starts_with("artwork/")
            && (name.to_ascii_lowercase().ends_with(".json")
                || name.to_ascii_lowercase().ends_with(".agc"));
        if !is_manifest && !is_artwork_json {
            if name.starts_with("resources/") && !name.ends_with("/") {
                const MAX_RESOURCE_BYTES: u64 = 100 * 1024 * 1024;
                if entry.size() > MAX_RESOURCE_BYTES {
                    return Err(AppError::InvalidXd(format!(
                        "ресурс {name} превышает 100 MiB"
                    )));
                }
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry
                    .read_to_end(&mut bytes)
                    .map_err(|source| AppError::Io {
                        path: PathBuf::from(&name),
                        source,
                    })?;
                resources.push(XdResource { name, bytes });
            }
            continue;
        }

        // Put a ceiling on individual JSON entries so a malformed archive cannot
        // force an unbounded allocation while it is being decoded.
        const MAX_JSON_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
        if entry.size() > MAX_JSON_ENTRY_BYTES {
            return Err(AppError::InvalidXd(format!(
                "JSON-файл {name} слишком велик ({} MiB)",
                entry.size() / (1024 * 1024)
            )));
        }

        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|source| AppError::Io {
                path: PathBuf::from(&name),
                source,
            })?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::InvalidXd(format!("повреждённый JSON в {name}: {error}")))?;

        if is_manifest {
            if manifest.replace(value).is_some() {
                return Err(AppError::InvalidXd(
                    "в архиве найдено несколько manifest файлов".into(),
                ));
            }
        } else {
            artwork_json.insert(name, value);
        }
    }

    let manifest = manifest.ok_or_else(|| AppError::InvalidXd("не найден manifest файл".into()))?;
    if artwork_json.is_empty() {
        return Err(AppError::InvalidXd(
            "не найдены JSON/.agc файлы в artwork/".into(),
        ));
    }

    let mut pages = Vec::new();
    for (archive_path, json) in &artwork_json {
        let artboard_path = archive_path
            .split('/')
            .find(|part| part.starts_with("artboard-"));
        let metadata = artboard_path.and_then(|wanted| find_manifest_artboard(&manifest, wanted));
        let page = parse_page(archive_path, json, metadata.as_ref());
        if !page.artboards.is_empty() {
            pages.push(page);
        }
    }
    if pages.is_empty() {
        return Err(AppError::InvalidXd(
            "в artwork/ не найдены артборды Adobe XD".into(),
        ));
    }
    let title = manifest_title(&manifest, path);
    let artwork = artwork_json
        .into_iter()
        .map(|(path, json)| XdArtworkFile { path, json })
        .collect();

    Ok(XdDocument {
        title,
        manifest,
        pages,
        artwork,
        resources,
    })
}

fn manifest_title(manifest: &Value, file_path: &Path) -> String {
    ["name", "title", "documentName"]
        .iter()
        .find_map(|key| manifest.get(key).and_then(Value::as_str).map(str::to_owned))
        .map(|name| name.trim_matches(char::is_control).trim().to_owned())
        .or_else(|| {
            file_path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "Untitled".into())
}

#[derive(Debug)]
struct ArtboardMetadata {
    name: Option<String>,
    bounds: Option<(f64, f64, f64, f64)>,
}

fn find_manifest_artboard(manifest: &Value, wanted: &str) -> Option<ArtboardMetadata> {
    fn visit(value: &Value, wanted: &str) -> Option<ArtboardMetadata> {
        match value {
            Value::Object(object) => {
                if value.get("path").and_then(Value::as_str) == Some(wanted) {
                    return Some(ArtboardMetadata {
                        name: string_field(value, &["name"]),
                        bounds: value.get("uxdesign#bounds").map(bounds),
                    });
                }
                object.values().find_map(|child| visit(child, wanted))
            }
            Value::Array(items) => items.iter().find_map(|child| visit(child, wanted)),
            _ => None,
        }
    }
    visit(manifest, wanted)
}

fn parse_page(path: &str, json: &Value, metadata: Option<&ArtboardMetadata>) -> XdPage {
    let page_name = metadata
        .and_then(|item| item.name.clone())
        .or_else(|| json.get("name").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| path.to_owned());
    let mut artboards = Vec::new();
    collect_artboards(json, metadata, &mut artboards);
    XdPage {
        name: page_name,
        artboards,
    }
}

fn collect_artboards(
    value: &Value,
    metadata: Option<&ArtboardMetadata>,
    out: &mut Vec<XdArtboard>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_artboards(item, metadata, out);
            }
        }
        Value::Object(object) => {
            if is_artboard(value) {
                out.push(parse_artboard(value, metadata));
            } else if value.get("type").and_then(Value::as_str) == Some("artboard")
                && value.get("artboard").is_some()
            {
                // XD stores an artboard's actual properties and child layers in the
                // sibling `artboard` object; retain that complete subtree as the raw node.
                let mut artboard = value.clone();
                let mut properties = value["artboard"].clone();
                if let (Some(target), Some(source)) =
                    (artboard.as_object_mut(), properties.as_object_mut())
                {
                    for (key, value) in source.iter() {
                        target.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
                artboard["type"] = Value::String("artboard".into());
                out.push(parse_artboard(&artboard, metadata));
            } else {
                for child in object.values() {
                    collect_artboards(child, metadata, out);
                }
            }
        }
        _ => {}
    }
}

fn is_artboard(value: &Value) -> bool {
    type_name(value).is_some_and(|kind| kind.eq_ignore_ascii_case("artboard"))
}

fn parse_artboard(raw: &Value, metadata: Option<&ArtboardMetadata>) -> XdArtboard {
    let content = raw.get("artboard").unwrap_or(raw);
    let bounds = metadata
        .and_then(|item| item.bounds)
        .unwrap_or_else(|| bounds(content));
    let mut children = Vec::new();
    if let Some(items) = child_nodes(content) {
        for item in items {
            collect_elements(item, &mut children);
        }
    }
    XdArtboard {
        id: string_field(raw, &["id", "_id"]),
        name: metadata
            .and_then(|item| item.name.clone())
            .or_else(|| string_field(raw, &["name", "label"]))
            .unwrap_or_else(|| "Artboard".into()),
        x: bounds.0,
        y: bounds.1,
        width: bounds.2,
        height: bounds.3,
        children,
        raw: content.clone(),
    }
}

fn collect_elements(value: &Value, out: &mut Vec<XdElement>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_elements(item, out);
            }
        }
        Value::Object(_) => {
            if is_artboard(value) {
                return;
            }
            // XD represents components/linked artwork as syncRef wrappers.
            // Their actual content lives under group.children; treating the
            // wrapper itself as a vector creates an invalid node without a
            // VectorNetwork. Preserve the wrapper as a group and recurse.
            if value.get("type").and_then(Value::as_str) == Some("syncRef") {
                let Some(children) = value.pointer("/group/children").and_then(Value::as_array)
                else {
                    return;
                };
                let mut normalized_children = Vec::new();
                for child in children {
                    collect_elements(child, &mut normalized_children);
                }
                let b = bounds(value);
                out.push(XdElement {
                    id: string_field(value, &["guid", "id", "_id"]),
                    name: string_field(value, &["name", "label"]).unwrap_or_else(|| "Component".into()),
                    kind: "group".into(),
                    x: b.0,
                    y: b.1,
                    width: b.2,
                    height: b.3,
                    text: None,
                    path_data: None,
                    raw: serde_json::json!({"type":"group", "transform":value.get("transform"), "normalized_children":normalized_children}),
                });
                return;
            }
            if let Some(kind) = type_name(value) {
                let lower = kind.to_ascii_lowercase();
                if lower.contains("group") {
                    let mut children = Vec::new();
                    if let Some(items) = child_nodes(value) {
                        for item in items {
                            collect_elements(item, &mut children);
                        }
                    }
                    let b = bounds(value);
                    out.push(XdElement {
                        id: string_field(value, &["id", "_id"]),
                        name: string_field(value, &["name", "label"])
                            .unwrap_or_else(|| "Group".into()),
                        kind: "group".into(),
                        x: b.0,
                        y: b.1,
                        width: b.2,
                        height: b.3,
                        text: None,
                        path_data: None,
                        raw: serde_json::json!({
                            "type":"group",
                            "transform":value.get("transform"),
                            "normalized_children":children
                        }),
                    });
                    return;
                }
            }
            let normalized_shape = value.get("type").and_then(Value::as_str) == Some("shape")
                && value.pointer("/shape/type").is_some();
            if normalized_shape {
                let b = bounds(value);
                let kind = value
                    .pointer("/shape/type")
                    .and_then(Value::as_str)
                    .unwrap_or("path");
                let mut raw = value["shape"].clone();
                if let (Some(target), Some(source)) = (raw.as_object_mut(), value.as_object()) {
                    for key in ["name", "id", "transform", "style"] {
                        if let Some(field) = source.get(key) {
                            target.insert(key.to_owned(), field.clone());
                        }
                    }
                }
                out.push(XdElement {
                    id: string_field(value, &["id", "_id"]),
                    name: string_field(value, &["name", "label"])
                        .unwrap_or_else(|| kind.to_owned()),
                    kind: normalize_kind(kind),
                    x: b.0,
                    y: b.1,
                    width: b.2,
                    height: b.3,
                    text: None,
                    path_data: value
                        .pointer("/shape/path")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    raw,
                });
                return;
            }
            if let Some(kind) = type_name(value) {
                // Only collect top-level elements here. Descendants are emitted
                // recursively by the FigCraft exporter to preserve groups.
                let text = value
                    .pointer("/text/rawText")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        value
                            .get("text")
                            .and_then(|v| v.as_str().map(str::to_owned))
                    })
                    .or_else(|| {
                        value
                            .pointer("/text/0/characters")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                let mut b = bounds(value);
                if let Some(text_value) = text.as_deref() {
                    let size = value
                        .pointer("/style/font/size")
                        .and_then(Value::as_f64)
                        .or_else(|| {
                            value
                                .pointer("/meta/ux/rangedStyles/0/fontSize")
                                .and_then(Value::as_f64)
                        })
                        .unwrap_or(14.0);
                    if b.2 <= 0.0 {
                        b.2 = text_value
                            .lines()
                            .map(|line| line.chars().count())
                            .max()
                            .unwrap_or(1) as f64
                            * size
                            * 0.6;
                    }
                    if b.3 <= 0.0 {
                        let last_y = value
                            .pointer("/text/paragraphs")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .flat_map(|paragraph| {
                                paragraph
                                    .get("lines")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                            })
                            .flat_map(|line| line.as_array().into_iter().flatten())
                            .filter_map(|run| run.get("y").and_then(Value::as_f64))
                            .fold(0.0_f64, f64::max);
                        b.3 = last_y + size * 1.35;
                    }
                }
                let path_data =
                    ["/shape/path", "/path", "/d", "/pathData"]
                        .iter()
                        .find_map(|key| {
                            value
                                .pointer(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        });
                let kind = value
                    .pointer("/shape/type")
                    .and_then(Value::as_str)
                    .unwrap_or(kind);
                let mut element = value.clone();
                if element.get("type").and_then(Value::as_str) == Some("shape") {
                    if let Some(shape) = element.get("shape") {
                        let mut actual = shape.clone();
                        if let (Some(target), Some(source)) =
                            (actual.as_object_mut(), value.as_object())
                        {
                            for key in ["name", "id", "transform", "style"] {
                                if let Some(field) = source.get(key) {
                                    target.insert(key.to_owned(), field.clone());
                                }
                            }
                        }
                        element = actual;
                    }
                }
                out.push(XdElement {
                    id: string_field(value, &["id", "_id"]),
                    name: string_field(value, &["name", "label"]).unwrap_or_else(|| kind.into()),
                    kind: normalize_kind(kind),
                    x: b.0,
                    y: b.1,
                    width: b.2,
                    height: b.3,
                    text,
                    path_data,
                    raw: element,
                });
            }
        }
        _ => {}
    }
}

fn child_nodes(value: &Value) -> Option<&Vec<Value>> {
    ["children", "layers", "objects", "nodes"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
        .or_else(|| {
            value
                .pointer("/artboard/children")
                .and_then(Value::as_array)
        })
        .or_else(|| value.pointer("/group/children").and_then(Value::as_array))
}

fn type_name(value: &Value) -> Option<&str> {
    ["type", "_class", "class", "nodeType"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn normalize_kind(kind: &str) -> String {
    let lower = kind.to_ascii_lowercase();
    if lower.contains("rect") {
        "rectangle".into()
    } else if lower.contains("text") {
        "text".into()
    } else if lower.contains("path") || lower.contains("shape") {
        "path".into()
    } else if lower.contains("group") {
        "group".into()
    } else {
        lower
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
}

/// Поддерживает распространённые варианты XD `bounds`, `frame`, `geometry` и отдельные поля.
fn bounds(value: &Value) -> (f64, f64, f64, f64) {
    let rect = ["bounds", "frame", "geometry", "rect"]
        .iter()
        .find_map(|key| value.get(*key).filter(|v| v.is_object()));
    let shape = value.get("shape").filter(|v| v.is_object());
    let text_frame = value.pointer("/text/frame").filter(|v| v.is_object());
    let transform = value.get("transform").filter(|v| v.is_object());
    let read_position = |axis: &str| -> f64 {
        rect.and_then(|r| r.get(axis))
            .or_else(|| transform.and_then(|r| r.get(if axis == "x" { "tx" } else { "ty" })))
            .or_else(|| shape.and_then(|r| r.get(axis)))
            .or_else(|| value.get(axis))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let read_size = |key: &str| -> f64 {
        rect.and_then(|r| r.get(key))
            .or_else(|| shape.and_then(|r| r.get(key)))
            .or_else(|| text_frame.and_then(|r| r.get(key)))
            .or_else(|| value.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    (
        read_position("x"),
        read_position("y"),
        read_size("width"),
        read_size("height"),
    )
}
