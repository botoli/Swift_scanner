# Architecture map

This project is organized by responsibility so an agent can load only the context needed for a task.

- `src/main.rs` — dependency imports, shared constants, module graph, and process entry point.
- `src/app.rs` — application state, event orchestration, immediate top-50 scan previews, and eframe screen composition.
- `src/model.rs` — shared domain and UI state types.
- `src/scanner.rs` — engine selection and portable `jwalk` fallback.
- `src/scanner/windows_ntfs.rs` — primary parallel Win32 `LARGE_FETCH` traversal plus optional MFT/USN enumeration, path reconstruction, and metadata fallback.
- `src/index.rs` — in-memory trigram postings, precomputed sort orders, and bounded page queries used by live search and table navigation.
- `src/visual_index.rs` — persistent perceptual-hash and MobileCLIP image indexing, ONNX inference, ANN search, and thumbnail decoding.
- `src/cleanup.rs` — cleanup classification and safety scoring.
- `src/filtering.rs` — shared file predicates and row ordering used by previews and index queries.
- `src/games.rs` — self-contained egui state, canvas rendering, and interactions for the always-available mini-game section.
- `src/ui.rs` — styling, display formatting, table helpers, and OS open/reveal actions.
- `src/tests.rs` — unit tests and opt-in filesystem benchmarks.

## Dependency direction

```text
main
 └─ app
    ├─ model
    ├─ scanner
    ├─ cleanup
    ├─ filtering
    ├─ games
    ├─ index
    ├─ visual_index
    └─ ui
```

Backend modules depend on `model` and shared constants, but not on `app`. Keep new filesystem work out of the UI module and keep egui widgets out of scanner/analysis modules.

## Change routing

- NTFS/MFT/USN performance: start in `scanner/windows_ntfs.rs`.
- Search latency or sort performance: start in `index.rs`.
- Similar-photo indexing, ML inference, image cache, or thumbnail decoding: start in `visual_index.rs`.
- Portable filesystem compatibility: start in `scanner.rs`.
- Cleanup recommendations and protected paths: start in `cleanup.rs`.
- Search, size filters, or sorting: start in `filtering.rs`.
- Screen behavior and async job lifecycle: start in `app.rs`.
- Labels, formatting, colors, or OS launch actions: start in `ui.rs`.
