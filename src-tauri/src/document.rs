use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{imageops, RgbaImage};
use serde::Serialize;
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};
use zip::ZipArchive;

use crate::clip_size::{clip_size, SMALL_PIXELS};

/**
 * Reads an Image Sage document (`.imagesage`): a zip archive with a
 * `manifest.json` and one PNG for each layer. This app only reads the
 * document; it never changes it and never writes a layer's picture to the
 * disk. A clip starts from the layer's own picture; where that picture is
 * transparent, the layers below it show.
 */
const DOCUMENT_FORMAT: &str = "imagesage-document";
/// Version 2 stores the original image as a separate tile; version 3 stores it as the bottom layer.
const OLDEST_LAYER_VERSION: u64 = 2;
const NEWEST_VERSION: u64 = 3;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_LAYERS: usize = 2000;
const THUMBNAIL_SIDE: u32 = 112;

/// One layer of a document, bottom layer first.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRef {
    /// The path of the layer's PNG in the archive.
    pub tile: String,
    pub name: String,
    pub prompt: String,
    /// The layer is turned off in Image Sage.
    pub hidden: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerInfo {
    tile: String,
    name: String,
    prompt: String,
    hidden: bool,
    /// A small JPEG of the layer, as a data URL.
    thumbnail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    path: String,
    width: u32,
    height: u32,
    /// The size of each clip; none when the document is wider or taller than 3:1.
    clip_width: Option<u32>,
    clip_height: Option<u32>,
    layers: Vec<LayerInfo>,
}

fn is_tile_path(path: &str) -> bool {
    path.strip_prefix("history/")
        .is_some_and(|name| !name.is_empty() && name.ends_with(".png") && !name.contains(['/', '\\']) && !name.contains(".."))
}

fn open_archive(path: &Path) -> Result<ZipArchive<File>, String> {
    let file = File::open(path).map_err(|error| format!("Could not open the document: {error}"))?;
    ZipArchive::new(file).map_err(|error| format!("Could not read the Image Sage document: {error}"))
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str, maximum: u64) -> Result<Vec<u8>, String> {
    let mut entry = archive.by_name(name).map_err(|_| format!("The Image Sage document is missing {name}"))?;
    if entry.size() > maximum {
        return Err(format!("{name} is too large to open safely"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).map_err(|error| format!("Could not read {name}: {error}"))?;
    Ok(bytes)
}

/// A layer's name as Image Sage shows it: the name the user gave it, or its prompt.
fn layer_name(name: Option<&str>, prompt: &str, index: usize) -> String {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let prompt_line = prompt.lines().next().unwrap_or_default().trim();
    match (name, prompt_line.is_empty(), index) {
        (Some(name), _, _) => name.to_string(),
        (None, false, _) => prompt_line.chars().take(80).collect(),
        (None, true, 0) => "Original".into(),
        (None, true, _) => format!("Layer {}", index + 1),
    }
}

/// The layers a manifest names, bottom layer first. `historyIndex` is only the layer that is selected in Image Sage, so every step is a layer.
fn manifest_layers(manifest: &Value) -> Result<Vec<LayerRef>, String> {
    if manifest.get("format").and_then(Value::as_str) != Some(DOCUMENT_FORMAT) {
        return Err("This is not an Image Sage document".into());
    }
    let version = manifest
        .get("formatVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| "The Image Sage document has no valid format version".to_string())?;
    if version > NEWEST_VERSION {
        return Err(format!("This document uses format version {version}, which is newer than this app. Update Image Sage Video."));
    }
    let too_old = || "This document has an old format with no layers. Open it in Image Sage and save it again.".to_string();
    if version < OLDEST_LAYER_VERSION {
        return Err(too_old());
    }
    let mut layers = Vec::new();
    let mut add = |tile: &str, name: Option<&str>, prompt: &str, hidden: bool| -> Result<(), String> {
        if !is_tile_path(tile) {
            return Err(format!("The document references an invalid layer file: {tile}"));
        }
        let index = layers.len();
        layers.push(LayerRef { tile: tile.to_string(), name: layer_name(name, prompt, index), prompt: prompt.trim().to_string(), hidden });
        Ok(())
    };
    if let Some(base) = manifest.get("base").and_then(Value::as_str).filter(|base| !base.is_empty()) {
        let origin = &manifest["origin"];
        let prompt = if origin["kind"] == "generated" { origin["prompt"].as_str().unwrap_or_default() } else { "" };
        add(base, Some("Original"), prompt, false)?;
    }
    let steps = manifest.get("history").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    if steps.len() > MAX_LAYERS {
        return Err("The Image Sage document has too many layers to open safely".into());
    }
    for step in steps {
        let tile = step.get("layer").and_then(Value::as_str).ok_or_else(too_old)?;
        add(
            tile,
            step.get("name").and_then(Value::as_str),
            step.get("prompt").and_then(Value::as_str).unwrap_or_default(),
            step.get("hidden").and_then(Value::as_bool).unwrap_or(false),
        )?;
    }
    if layers.is_empty() {
        return Err("The Image Sage document has no layers".into());
    }
    Ok(layers)
}

fn read_manifest(archive: &mut ZipArchive<File>) -> Result<Vec<LayerRef>, String> {
    let bytes = read_entry(archive, "manifest.json", MAX_MANIFEST_BYTES)?;
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|error| format!("The Image Sage document manifest is invalid: {error}"))?;
    manifest_layers(&manifest)
}

fn read_layer(archive: &mut ZipArchive<File>, tile: &str) -> Result<RgbaImage, String> {
    let bytes = read_entry(archive, tile, MAX_IMAGE_BYTES)?;
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map(|picture| picture.into_rgba8())
        .map_err(|error| format!("Could not decode {tile}: {error}"))
}

/// The layers of a document, bottom layer first.
pub fn read_layers(path: &Path) -> Result<Vec<LayerRef>, String> {
    read_manifest(&mut open_archive(path)?)
}

/// The document's size: the size of its bottom layer, read from the PNG header.
fn document_size(archive: &mut ZipArchive<File>, layers: &[LayerRef]) -> Result<(u32, u32), String> {
    let bytes = read_entry(archive, &layers[0].tile, MAX_IMAGE_BYTES)?;
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("Could not read the document size: {error}"))?
        .into_dimensions()
        .map_err(|error| format!("Could not read the document size: {error}"))
}

/// The size of a document's clips. Every clip has the document's shape, so the clips can be joined.
pub fn document_clip_size(path: &Path, layers: &[LayerRef]) -> Result<(u32, u32), String> {
    let (width, height) = document_size(&mut open_archive(path)?, layers)?;
    clip_size(width, height, SMALL_PIXELS).ok_or_else(|| "The document is wider or taller than 3:1; Wan 2.2 cannot make a clip of that shape.".to_string())
}

/// Puts `layers` (bottom first) on a black picture of the document's size.
fn flatten(width: u32, height: u32, layers: Vec<RgbaImage>) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    for layer in layers {
        let layer = if layer.dimensions() == (width, height) { layer } else { imageops::resize(&layer, width, height, imageops::FilterType::Lanczos3) };
        imageops::overlay(&mut canvas, &layer, 0, 0);
    }
    canvas
}

/**
 * The first picture of a layer's clips, as a PNG data URL at the clip size.
 * It is the layer's own picture, without its masks and adjustments. Where
 * the layer is transparent, the layers below it show.
 */
pub fn layer_start_picture(path: &Path, layers: &[LayerRef], index: usize, width: u32, height: u32) -> Result<String, String> {
    let mut archive = open_archive(path)?;
    let mut stack = Vec::new();
    for layer in layers[..=index].iter().rev() {
        let picture = read_layer(&mut archive, &layer.tile)?;
        let opaque = picture.pixels().all(|pixel| pixel[3] == 255);
        stack.push(picture);
        if opaque {
            break;
        }
    }
    stack.reverse();
    let (doc_width, doc_height) = document_size(&mut archive, layers)?;
    let flat = flatten(doc_width, doc_height, stack);
    let clip = imageops::resize(&flat, width, height, imageops::FilterType::Lanczos3);
    let rgb = image::DynamicImage::ImageRgba8(clip).into_rgb8();
    let mut png = Vec::new();
    rgb.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("Could not prepare the layer's picture: {error}"))?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn thumbnail(picture: &RgbaImage) -> Result<String, String> {
    let small = imageops::thumbnail(picture, THUMBNAIL_SIDE * 2, THUMBNAIL_SIDE);
    let mut canvas = RgbaImage::from_pixel(small.width(), small.height(), image::Rgba([0x1d, 0x20, 0x29, 255]));
    imageops::overlay(&mut canvas, &small, 0, 0);
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgba8(canvas)
        .into_rgb8()
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .map_err(|error| format!("Could not make a layer's small picture: {error}"))?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg)))
}

fn document_info(path: &Path) -> Result<DocumentInfo, String> {
    let mut archive = open_archive(path)?;
    let layers = read_manifest(&mut archive)?;
    let mut size = (0, 0);
    let mut infos = Vec::with_capacity(layers.len());
    for (index, layer) in layers.into_iter().enumerate() {
        let picture = read_layer(&mut archive, &layer.tile)?;
        if index == 0 {
            size = picture.dimensions();
        }
        infos.push(LayerInfo { thumbnail: thumbnail(&picture)?, tile: layer.tile, name: layer.name, prompt: layer.prompt, hidden: layer.hidden });
    }
    let clip = clip_size(size.0, size.1, SMALL_PIXELS);
    Ok(DocumentInfo {
        path: path.to_string_lossy().into_owned(),
        width: size.0,
        height: size.1,
        clip_width: clip.map(|(width, _)| width),
        clip_height: clip.map(|(_, height)| height),
        layers: infos,
    })
}

/// Reads a document's layers with a small picture of each, on a worker thread.
#[tauri::command]
pub async fn open_document(path: String) -> Result<DocumentInfo, String> {
    tauri::async_runtime::spawn_blocking(move || document_info(Path::new(&path)))
        .await
        .map_err(|error| format!("Reading the document stopped unexpectedly: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(width, height, image::Rgba(color))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn write_document(label: &str, manifest: &Value, tiles: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("imagesagevideo-{label}-{}.imagesage", std::process::id()));
        let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
        archive.start_file("manifest.json", SimpleFileOptions::default()).unwrap();
        archive.write_all(manifest.to_string().as_bytes()).unwrap();
        for (name, bytes) in tiles {
            archive.start_file(*name, SimpleFileOptions::default()).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        path
    }

    fn decode(data_url: &str) -> RgbaImage {
        let bytes = STANDARD.decode(data_url.split_once(',').unwrap().1).unwrap();
        image::load_from_memory(&bytes).unwrap().into_rgba8()
    }

    #[test]
    fn lists_the_layers_of_a_version_3_document() {
        let manifest = json!({
            "format": "imagesage-document", "formatVersion": 3, "historyIndex": 2,
            "history": [
                { "prompt": "", "layer": "history/0001-layer.png" },
                { "prompt": "add a red hat\nwith a feather", "layer": "history/0002-layer.png", "hidden": true },
                { "prompt": "above the selected layer", "layer": "history/0003-layer.png" }
            ]
        });
        let layers = manifest_layers(&manifest).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!((layers[0].name.as_str(), layers[0].prompt.as_str()), ("Original", ""));
        assert_eq!((layers[1].name.as_str(), layers[1].hidden), ("add a red hat", true));
    }

    #[test]
    fn a_version_2_document_has_its_original_image_as_the_bottom_layer() {
        let manifest = json!({
            "format": "imagesage-document", "formatVersion": 2, "base": "history/base.png",
            "origin": { "kind": "generated", "prompt": "a fox in snow" },
            "history": [{ "prompt": "make it night", "name": "Night", "layer": "history/0001-layer.png" }]
        });
        let layers = manifest_layers(&manifest).unwrap();
        assert_eq!(layers[0], LayerRef { tile: "history/base.png".into(), name: "Original".into(), prompt: "a fox in snow".into(), hidden: false });
        assert_eq!(layers[1].name, "Night");
    }

    #[test]
    fn refuses_documents_it_cannot_use() {
        let old = json!({ "format": "imagesage-document", "formatVersion": 1, "history": [] });
        assert!(manifest_layers(&old).unwrap_err().contains("old format"));
        let tiles = json!({ "format": "imagesage-document", "formatVersion": 3, "history": [{ "prompt": "x", "before": "history/0001-before.png", "after": "history/0001-after.png" }] });
        assert!(manifest_layers(&tiles).is_err());
        let outside = json!({ "format": "imagesage-document", "formatVersion": 3, "history": [{ "prompt": "x", "layer": "history/../secret.png" }] });
        assert!(manifest_layers(&outside).is_err());
        assert!(manifest_layers(&json!({ "format": "other" })).is_err());
    }

    #[test]
    fn a_transparent_layer_shows_the_layer_below_it() {
        let manifest = json!({
            "format": "imagesage-document", "formatVersion": 3,
            "history": [
                { "prompt": "", "layer": "history/0001-layer.png" },
                { "prompt": "clear", "layer": "history/0002-layer.png" },
                { "prompt": "green", "layer": "history/0003-layer.png" }
            ]
        });
        let path = write_document("flatten", &manifest, &[
            ("history/0001-layer.png", png(64, 32, [255, 0, 0, 255])),
            ("history/0002-layer.png", png(64, 32, [0, 0, 0, 0])),
            ("history/0003-layer.png", png(64, 32, [0, 255, 0, 255])),
        ]);
        let layers = read_layers(&path).unwrap();
        let through = decode(&layer_start_picture(&path, &layers, 1, 32, 16).unwrap());
        assert_eq!(through.dimensions(), (32, 16));
        assert_eq!(through.get_pixel(5, 5).0, [255, 0, 0, 255]);
        let top = decode(&layer_start_picture(&path, &layers, 2, 32, 16).unwrap());
        assert_eq!(top.get_pixel(5, 5).0, [0, 255, 0, 255]);
        std::fs::remove_file(path).unwrap();
    }
}
