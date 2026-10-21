# XD to FigCraft

A small macOS desktop utility that converts Adobe XD (`.xd`) documents into FigCraft (`.figcraft`) files. The application runs locally and does not upload design files.

> **Format note:** this project exports FigCraft's `.figcraft` format. It does not create Figma's native `.fig` files.

## Features

- Select an `.xd` file with the native file picker or drag it into the app.
- Choose where to save the converted `.figcraft` document.
- Parse XD ZIP contents in memory and write a FigCraft ZIP archive.
- Preserve common layout nodes, vector paths, text styling ranges, and embedded image resources.

## Requirements

- macOS with Xcode Command Line Tools
- Rust and Cargo
- Node.js and npm

## Run in development

From the project root:

```sh
npm install
npm run tauri dev
```

## Build the desktop app

```sh
npm run tauri build
```

Tauri writes platform bundles and build artifacts under `src-tauri/target/release/bundle/`.

## Verify the Rust converter

```sh
cargo test --manifest-path src-tauri/Cargo.toml
```

The integration tests also convert the sample files in `temp/` when those files are present. The `website.xd` sample is large, so its test takes longer than the smaller fixtures.

## Current limitations

XD and FigCraft use different document models, so conversion is best-effort. Linked XD component references (`syncRef`) are preserved as empty groups instead of being recursively expanded. Unsupported vector paths are emitted with an empty vector network so the FigCraft document remains structurally valid, but their artwork will not appear. Fonts are referenced by family and style; the corresponding fonts need to be installed on the system for FigCraft to render them as intended.

The converter has been checked against the included sample archives. Review the result in FigCraft, especially when a document uses linked components, uncommon vector operations, or fonts unavailable on the target Mac.

## Project layout

```text
src/                         Vanilla TypeScript UI and CSS
src-tauri/src/parser.rs      Adobe XD archive and document parser
src-tauri/src/figcraft_exporter.rs  FigCraft document builder and ZIP exporter
src-tauri/src/converter.rs   Conversion orchestration and integration tests
src-tauri/src/lib.rs         Tauri command registration
```
