use std::collections::HashSet;

use serde_json::Value;

use crate::{
    error::{AppError, Result},
    parser::{XdArtboard, XdDocument},
};

/// Build an SVG document with one editable SVG group per XD artboard.
pub fn document_to_svg(document: &XdDocument) -> Result<String> {
    let artboards: Vec<&XdArtboard> = document
        .pages
        .iter()
        .flat_map(|page| page.artboards.iter())
        .collect();
    if artboards.is_empty() {
        return Err(AppError::InvalidXd(
            "в документе нет артбордов для экспорта".into(),
        ));
    }

    let width = artboards
        .iter()
        .map(|board| board.width.max(1.0))
        .fold(1.0, f64::max);
    let gap = 48.0;
    let height: f64 = artboards
        .iter()
        .map(|board| board.height.max(1.0) + gap)
        .sum::<f64>()
        - gap;
    let mut svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n<title>{}</title>\n",
        number(width), number(height), number(width), number(height), escape_xml(&document.title)
    );

    let mut offset_y = 0.0;
    let mut used_ids = HashSet::new();
    for board in &artboards {
        let group_id = unique_id(&sanitize_id(&board.name), &mut used_ids);
        let clip_id = format!("clip-{}", group_id);
        svg.push_str(&format!(
            "<defs><clipPath id=\"{}\" clipPathUnits=\"userSpaceOnUse\"><rect width=\"{}\" height=\"{}\" /></clipPath></defs>\n<g id=\"{}\" aria-label=\"{}\" transform=\"translate(0 {})\">\n<g clip-path=\"url(#{})\">\n<g transform=\"translate({} {})\">\n",
            clip_id,
            number(board.width),
            number(board.height),
            group_id,
            escape_xml(&board.name),
            number(offset_y),
            clip_id,
            number(-board.x),
            number(-board.y)
        ));

        if let Some(children) = board
            .raw
            .pointer("/artboard/children")
            .and_then(Value::as_array)
        {
            for child in children {
                render_node(child, &mut svg, &mut used_ids);
            }
        } else {
            for child in &board.children {
                render_node(&child.raw, &mut svg, &mut used_ids);
            }
        }
        svg.push_str("</g>\n</g>\n</g>\n");
        offset_y += board.height.max(1.0) + gap;
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

fn render_node(node: &Value, svg: &mut String, used_ids: &mut HashSet<String>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if kind == "artboard" {
        return;
    }

    let shape_kind = node
        .pointer("/shape/type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let group_children = node
        .pointer("/group/children")
        .and_then(Value::as_array)
        .or_else(|| node.get("children").and_then(Value::as_array));
    let name = node.get("name").and_then(Value::as_str).unwrap_or(kind);
    let id = unique_id(&sanitize_id(name), used_ids);
    let source_id = node.get("id").and_then(Value::as_str).unwrap_or_default();
    let transform = svg_transform(node.get("transform"));
    svg.push_str(&format!(
        "<g id=\"{}\" data-xd-id=\"{}\"",
        escape_xml(&id),
        escape_xml(source_id)
    ));
    if !transform.is_empty() {
        svg.push_str(&format!(" transform=\"{}\"", transform));
    }
    svg.push_str("><title>");
    svg.push_str(&escape_xml(name));
    svg.push_str("</title>\n");

    if kind == "group" || !group_children.is_none() {
        if let Some(children) = group_children {
            for child in children {
                render_node(child, svg, used_ids);
            }
        }
    } else if kind == "shape" {
        render_shape(node, shape_kind, svg);
    } else if kind == "text" {
        render_text(node, svg);
    }
    svg.push_str("</g>\n");
}

fn render_shape(node: &Value, shape_kind: &str, svg: &mut String) {
    let shape = match node.get("shape") {
        Some(shape) => shape,
        None => return,
    };
    let fill = paint(node.pointer("/style/fill"), "#000000");
    let stroke = paint(node.pointer("/style/stroke"), "none");
    let stroke_width = node
        .pointer("/style/stroke/width")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let style = format!(
        " fill=\"{}\" fill-opacity=\"{}\" stroke=\"{}\" stroke-opacity=\"{}\" stroke-width=\"{}\"",
        fill,
        number(paint_alpha(node.pointer("/style/fill"))),
        stroke,
        number(paint_alpha(node.pointer("/style/stroke"))),
        number(stroke_width)
    );

    match shape_kind {
        "rect" | "rectangle" => {
            let x = numeric(shape.get("x"));
            let y = numeric(shape.get("y"));
            let width = numeric(shape.get("width"));
            let height = numeric(shape.get("height"));
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{} />\n",
                number(x),
                number(y),
                number(width),
                number(height),
                style
            ));
        }
        "path" => {
            if let Some(path) = shape.get("path").and_then(Value::as_str) {
                svg.push_str(&format!("<path d=\"{}\"{} />\n", escape_xml(path), style));
            }
        }
        "ellipse" => {
            let x = numeric(shape.get("x"));
            let y = numeric(shape.get("y"));
            let width = numeric(shape.get("width"));
            let height = numeric(shape.get("height"));
            svg.push_str(&format!(
                "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"{} />\n",
                number(x + width / 2.0),
                number(y + height / 2.0),
                number(width / 2.0),
                number(height / 2.0),
                style
            ));
        }
        _ => {}
    }
}

fn render_text(node: &Value, svg: &mut String) {
    let Some(text) = node.pointer("/text/rawText").and_then(Value::as_str) else {
        return;
    };
    let font_size = node
        .pointer("/style/font/size")
        .and_then(Value::as_f64)
        .unwrap_or(16.0);
    let family = node
        .pointer("/style/font/family")
        .and_then(Value::as_str)
        .unwrap_or("sans-serif");
    let font_style = node
        .pointer("/style/font/style")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let font_weight = weight_from_style(font_style);
    let fill = paint(node.pointer("/style/fill"), "#000000");
    svg.push_str(&format!(
        "<text x=\"0\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\" fill=\"{}\" fill-opacity=\"{}\">",
        number(text_line_baselines(node).first().copied().unwrap_or(font_size)),
        escape_xml(family),
        number(font_size),
        font_weight,
        fill,
        number(paint_alpha(node.pointer("/style/fill")))
    ));
    let baselines = text_line_baselines(node);
    for (index, line) in text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .enumerate()
    {
        let dy = if index == 0 {
            "0".to_owned()
        } else if let (Some(current), Some(previous)) =
            (baselines.get(index), baselines.get(index - 1))
        {
            number((current - previous).abs())
        } else {
            number(font_size * 1.2)
        };
        svg.push_str(&format!(
            "<tspan x=\"0\" dy=\"{}\">{}</tspan>",
            dy,
            escape_xml(line)
        ));
    }
    svg.push_str("</text>\n");
}

fn paint(paint: Option<&Value>, fallback: &str) -> String {
    let Some(paint) = paint else {
        return fallback.to_owned();
    };
    if paint.get("type").and_then(Value::as_str) == Some("none") {
        return "none".into();
    }
    let color = paint.get("color").unwrap_or(paint);
    let value = color.get("value").unwrap_or(color);
    let (Some(red), Some(green), Some(blue)) = (
        value.get("r").and_then(Value::as_u64),
        value.get("g").and_then(Value::as_u64),
        value.get("b").and_then(Value::as_u64),
    ) else {
        return fallback.to_owned();
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        red.min(255),
        green.min(255),
        blue.min(255)
    )
}

fn paint_alpha(paint: Option<&Value>) -> f64 {
    paint
        .and_then(|paint| {
            paint
                .get("color")
                .and_then(|color| color.get("alpha"))
                .or_else(|| paint.get("alpha"))
        })
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
}

fn svg_transform(transform: Option<&Value>) -> String {
    let Some(transform) = transform else {
        return String::new();
    };
    let values = ["a", "b", "c", "d", "tx", "ty"].map(|key| {
        transform
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(if key == "a" || key == "d" { 1.0 } else { 0.0 })
    });
    format!("matrix({})", values.map(number).join(" "))
}

fn numeric(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn sanitize_id(value: &str) -> String {
    let id: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if id.is_empty() {
        "layer".into()
    } else {
        id
    }
}

fn unique_id(base: &str, used_ids: &mut HashSet<String>) -> String {
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    while !used_ids.insert(candidate.clone()) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

fn text_line_baselines(node: &Value) -> Vec<f64> {
    node.pointer("/text/paragraphs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|paragraph| paragraph.get("lines").and_then(Value::as_array))
        .flatten()
        .filter_map(|line| line.as_array()?.first()?.get("y")?.as_f64())
        .collect()
}

fn weight_from_style(style: &str) -> u16 {
    let style = style.to_ascii_lowercase();
    if style.contains("black") || style.contains("heavy") {
        900
    } else if style.contains("extra bold") || style.contains("extrabold") {
        800
    } else if style.contains("bold") {
        700
    } else if style.contains("semi") {
        600
    } else if style.contains("medium") {
        500
    } else if style.contains("light") {
        300
    } else {
        400
    }
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
    {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
