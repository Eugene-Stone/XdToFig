use std::path::PathBuf;

use crate::{error::Result, figcraft_exporter, parser};

pub fn convert(input_path: PathBuf, output_path: PathBuf) -> Result<()> {
    let document = parser::parse_xd_file(&input_path)?;
    figcraft_exporter::export(&document, &output_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use zip::ZipArchive;

    // Mirror FigCraft's public VectorNetwork/Region serde contract so a regression
    // fails here instead of producing another archive rejected by its document reader.
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    enum WindingRule {
        #[default]
        NonZero,
        EvenOdd,
    }

    #[derive(Deserialize)]
    struct FigCraftRegion {
        loops: Vec<Vec<usize>>,
        #[serde(default)]
        winding: WindingRule,
    }

    #[derive(Deserialize)]
    struct FigCraftVertex {
        x: f64,
        y: f64,
        #[serde(default)]
        radius: f64,
    }

    #[derive(Deserialize)]
    struct FigCraftSegment {
        a: usize,
        b: usize,
        #[serde(default)]
        ta: (f64, f64),
        #[serde(default)]
        tb: (f64, f64),
    }

    #[derive(Deserialize)]
    struct FigCraftVectorNetwork {
        vertices: Vec<FigCraftVertex>,
        segments: Vec<FigCraftSegment>,
        #[serde(default)]
        regions: Vec<FigCraftRegion>,
    }

    #[test]
    fn converts_sample_xd_to_figcraft_archive() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let input = root.join("temp/test.xd");
        if !input.exists() {
            return;
        }
        let keep_export = std::env::var_os("KEEP_XD_TEST_EXPORTS").is_some();
        let output = if keep_export {
            root.join("temp/test.figcraft")
        } else {
            std::env::temp_dir().join(format!("xd-to-figcraft-{}.figcraft", std::process::id()))
        };
        convert(input, output.clone()).unwrap();
        let file = std::fs::File::open(&output).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut json_file = archive.by_name("document.json").unwrap();
        let value: serde_json::Value = serde_json::from_reader(&mut json_file).unwrap();
        assert_eq!(value["format"], "figcraft");
        assert_eq!(value["version"], 1);
        assert!(value["styles"].is_array());
        assert!(value["nodes"].as_object().unwrap().len() > 1);
        assert!(value["nodes"]
            .as_object()
            .unwrap()
            .values()
            .any(|node| node["type"] == "text"));
        let nodes = value["nodes"].as_object().unwrap();
        assert!(
            nodes
                .values()
                .filter(|node| node["type"] == "vector")
                .count()
                >= 10
        );
        assert!(nodes.values().any(|node| node["type"] == "group"
            && node["children"]
                .as_array()
                .is_some_and(|children| !children.is_empty())));
        let artboard = nodes.values().find(|node| node["type"] == "frame").unwrap();
        assert_eq!(artboard["transform"][4], 0.0);
        assert_eq!(artboard["transform"][5], 0.0);
        let background = nodes
            .values()
            .find(|node| node["name"] == "Rechteck 240")
            .unwrap();
        assert_eq!(background["transform"][4], 0.0);
        assert_eq!(background["transform"][5], 0.0);
        assert!(nodes.values().any(|node| node["type"] == "vector"
            && node
                .pointer("/network/segments")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|segments| !segments.is_empty())));
        assert!(nodes.values().any(|node| {
            node["type"] == "vector"
                && node
                    .pointer("/network/regions")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|regions| !regions.is_empty())
                && node
                    .pointer("/network/segments")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|segments| {
                        segments.iter().any(|segment| {
                            segment["ta"][0].as_f64().unwrap_or(0.0) != 0.0
                                || segment["ta"][1].as_f64().unwrap_or(0.0) != 0.0
                                || segment["tb"][0].as_f64().unwrap_or(0.0) != 0.0
                                || segment["tb"][1].as_f64().unwrap_or(0.0) != 0.0
                        })
                    })
        }));
        let closed_curved_contours = nodes
            .values()
            .filter(|node| {
                node["type"] == "vector"
                    && node
                        .pointer("/network/regions/0/loops/0")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|loop_segments| !loop_segments.is_empty())
                    && node
                        .pointer("/network/segments")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|segments| {
                            segments.iter().any(|segment| {
                                segment["ta"][0].as_f64().unwrap_or(0.0) != 0.0
                                    || segment["ta"][1].as_f64().unwrap_or(0.0) != 0.0
                                    || segment["tb"][0].as_f64().unwrap_or(0.0) != 0.0
                                    || segment["tb"][1].as_f64().unwrap_or(0.0) != 0.0
                            })
                        })
            })
            .count();
        assert!(
            closed_curved_contours >= 3,
            "expected filled curved logo glyphs"
        );
        for node in nodes.values().filter(|node| node["type"] == "vector") {
            let network: FigCraftVectorNetwork =
                serde_json::from_value(node["network"].clone()).unwrap();
            assert!(network.vertices.iter().all(|vertex| {
                vertex.x.is_finite() && vertex.y.is_finite() && vertex.radius.is_finite()
            }));
            assert!(network.segments.iter().all(|segment| {
                segment.a < network.vertices.len()
                    && segment.b < network.vertices.len()
                    && segment.ta.0.is_finite()
                    && segment.ta.1.is_finite()
                    && segment.tb.0.is_finite()
                    && segment.tb.1.is_finite()
            }));
            for region in &network.regions {
                let _ = &region.winding;
                assert!(!region.loops.is_empty());
                assert!(region.loops.iter().all(|loop_segments| {
                    !loop_segments.is_empty()
                        && loop_segments
                            .iter()
                            .all(|index| *index < network.segments.len())
                }));
            }
        }
        if !keep_export {
            std::fs::remove_file(output).unwrap();
        }
    }

    #[test]
    fn converts_test_2_sample() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let input = root.join("temp/test_2.xd");
        if !input.exists() {
            return;
        }
        let output = root.join("temp/test_2.figcraft");
        convert(input, output.clone()).unwrap();
        let mut archive = ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
        let value: serde_json::Value = {
            let mut file = archive.by_name("document.json").unwrap();
            serde_json::from_reader(&mut file).unwrap()
        };
        let nodes = value["nodes"].as_object().unwrap();
        assert_eq!(value["format"], "figcraft");
        assert!(value["styles"].is_array());
        assert_eq!(
            nodes.values().filter(|node| node["type"] == "text").count(),
            2
        );
        let heading = nodes
            .values()
            .find(|node| node["name"] == "Tgea da Schons")
            .unwrap();
        assert_eq!(heading["style"]["font_family"], "Gilroy");
        assert_eq!(heading["style"]["font_style"], "ExtraBold");
        assert_eq!(heading["style"]["font_size"], 39.0);
        let body = nodes
            .values()
            .find(|node| node["type"] == "text" && node["name"] != "Tgea da Schons")
            .unwrap();
        assert_eq!(body["style"]["font_family"], "Times New Roman");
        assert_eq!(body["style"]["font_style"], "Italic");
        assert_eq!(body["style"]["font_size"], 27.0);
        let runs = body["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 4);
        assert_eq!(runs[0]["start"], 39);
        assert_eq!(runs[0]["end"], 40);
        assert_eq!(runs[0]["style"]["font_family"], "Dolly Pro");
        assert_eq!(runs[1]["start"], 40);
        assert_eq!(runs[1]["end"], 66);
        assert_eq!(runs[1]["style"]["font_family"], "Arial");
        assert_eq!(runs[1]["style"]["font_size"], 19.0);
        assert_eq!(runs[1]["style"]["decoration"], "underline");
        assert_eq!(runs[2]["start"], 66);
        assert_eq!(runs[2]["end"], 75);
        assert_eq!(runs[3]["start"], 75);
        assert_eq!(runs[3]["end"], 97);
        assert_eq!(runs[3]["style"]["decoration"], "underline");
        let background = nodes
            .values()
            .find(|node| node["type"] == "rectangle")
            .unwrap();
        assert_eq!(background["fills"][0]["type"], "image");
        assert_eq!(
            background["fills"][0]["image"],
            "3b40199980098f3acbd20327a31a353368cf9749f334540498f20dd0edc38890"
        );
        assert!(archive.file_names().any(|name| name.is_ok_and(|name| name
            == "images/3b40199980098f3acbd20327a31a353368cf9749f334540498f20dd0edc38890")));
        assert_eq!(
            value["imageIndex"][0]["hash"],
            background["fills"][0]["image"]
        );
    }

    #[test]
    fn website_vectors_always_have_networks() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let input = root.join("temp/website.xd");
        if !input.exists() {
            return;
        }
        let output = std::env::temp_dir().join(format!(
            "xd-to-figcraft-website-{}.figcraft",
            std::process::id()
        ));
        convert(input, output.clone()).unwrap();
        let mut archive = ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
        let value: serde_json::Value = {
            let mut file = archive.by_name("document.json").unwrap();
            serde_json::from_reader(&mut file).unwrap()
        };
        let nodes = value["nodes"].as_object().unwrap();
        let vectors: Vec<_> = nodes
            .values()
            .filter(|node| node["type"] == "vector")
            .collect();
        assert!(!vectors.is_empty());
        assert!(
            vectors.iter().all(|node| node.get("network").is_some()),
            "every vector node must include a VectorNetwork"
        );
        assert!(nodes
            .values()
            .filter(|node| node["type"] == "group")
            .any(|node| node["name"] == "Component"));
        std::fs::remove_file(output).unwrap();
    }
}
