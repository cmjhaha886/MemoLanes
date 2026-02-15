pub mod test_utils;
use memolanes_core::{journey_bitmap::JourneyBitmap, journey_data, renderer::map_renderer::LazyTileSource, renderer::*};
use std::time::Instant;

#[macro_use]
extern crate assert_float_eq;

#[test]
fn basic() {
    let mut journey_bitmap = JourneyBitmap::new();
    let start_lng = 151.1435370795134;
    let start_lat = -33.793291910360125;
    let end_lng = 151.2783692841415;
    let end_lat = -33.943600147192235;
    journey_bitmap.add_line(start_lng, start_lat, end_lng, end_lat);

    let mut map_renderer = MapRenderer::new(journey_bitmap);

    let render_result =
        test_utils::render_map_overlay(&mut map_renderer, 11, start_lng, start_lat, end_lng, end_lat);
    assert_f64_near!(render_result.left, 150.8203125);
    assert_f64_near!(render_result.top, -33.578014746143985);
    assert_f64_near!(render_result.right, 151.5234375);
    assert_f64_near!(render_result.bottom, -34.16181816123038);

    test_utils::verify_image("map_renderer_basic", &render_result.data);
}

#[test]
fn lazy_loading_correctness_and_performance() {
    // 1. Build a bitmap with lines spread across many geographic regions
    //    so the bitmap contains many distinct tiles.
    let mut journey_bitmap = JourneyBitmap::new();
    let regions: Vec<(f64, f64, f64, f64)> = vec![
        (151.14, -33.79, 151.28, -33.94), // Sydney
        (139.70, 35.60, 139.85, 35.75),   // Tokyo
        (-0.13, 51.48, 0.02, 51.53),      // London
        (-74.01, 40.70, -73.86, 40.85),   // New York
        (-43.20, -22.90, -43.05, -22.75), // Rio
        (116.35, 39.85, 116.50, 40.00),   // Beijing
        (77.15, 28.55, 77.30, 28.70),     // Delhi
        (2.30, 48.83, 2.45, 48.88),       // Paris
        (37.55, 55.70, 37.70, 55.85),     // Moscow
        (-118.30, 33.95, -118.15, 34.10), // Los Angeles
        (103.80, 1.25, 103.95, 1.40),     // Singapore
        (-46.60, -23.55, -46.45, -23.40), // Sao Paulo
        (174.70, -36.85, 174.85, -36.70), // Auckland
        (28.95, 41.00, 29.10, 41.15),     // Istanbul
        (100.50, 13.70, 100.65, 13.85),   // Bangkok
    ];
    for (start_lng, start_lat, end_lng, end_lat) in &regions {
        // Draw a diagonal and a cross-diagonal in each region
        journey_bitmap.add_line(*start_lng, *start_lat, *end_lng, *end_lat);
        journey_bitmap.add_line(*start_lng, *end_lat, *end_lng, *start_lat);
    }

    let total_tiles = journey_bitmap.tiles.len();
    println!("Total tiles in bitmap: {}", total_tiles);
    assert!(
        total_tiles >= 10,
        "Need multiple tiles for a meaningful lazy loading test, got {}",
        total_tiles
    );

    // 2. Serialize the bitmap
    let mut serialized = Vec::new();
    journey_data::serialize_journey_bitmap(&journey_bitmap, &mut serialized).unwrap();
    println!("Serialized bitmap size: {} bytes", serialized.len());

    // 3. Eager path: full deserialization
    let eager_start = Instant::now();
    let eager_bitmap =
        journey_data::deserialize_journey_bitmap(serialized.as_slice()).unwrap();
    let eager_init_duration = eager_start.elapsed();
    let mut eager_renderer = MapRenderer::new(eager_bitmap);

    // 4. Lazy path: only parse tile index headers, no decompression
    let lazy_start = Instant::now();
    let lazy_source = LazyTileSource::from_serialized_bitmap(serialized.clone()).unwrap();
    let lazy_init_duration = lazy_start.elapsed();
    let mut lazy_renderer = MapRenderer::new(JourneyBitmap::new());
    lazy_renderer.replace_lazy(lazy_source, JourneyBitmap::new());

    println!(
        "Eager init (full deserialization): {:?}",
        eager_init_duration
    );
    println!(
        "Lazy init (header parsing only):   {:?}",
        lazy_init_duration
    );

    // --- Performance assertion: lazy init should be faster ---
    assert!(
        lazy_init_duration < eager_init_duration,
        "Lazy init ({:?}) should be faster than eager init ({:?}) \
         because it skips zstd decompression of all tiles",
        lazy_init_duration,
        eager_init_duration
    );

    // --- Correctness: no tiles loaded initially in lazy mode ---
    assert_eq!(
        lazy_renderer.peek_latest_bitmap().tiles.len(),
        0,
        "No tiles should be loaded initially in lazy mode"
    );

    // 5. Request tile buffer for a small viewport: Sydney area at zoom 11
    //    At zoom 11, Sydney (~lng 151.2, lat -33.9) maps to roughly tile (1884, 1228).
    //    This should only decompress the Sydney-area bitmap tile(s).
    let lazy_tile_buf = lazy_renderer
        .get_tile_buffer(1884, 1228, 11, 2, 2, 9)
        .unwrap();

    let loaded_after_viewport = lazy_renderer.peek_latest_bitmap().tiles.len();
    println!(
        "Tiles loaded after Sydney viewport request: {} / {}",
        loaded_after_viewport, total_tiles
    );

    // --- Only tiles needed for the viewport should be loaded ---
    assert!(
        loaded_after_viewport > 0,
        "At least one tile should be loaded after requesting a viewport"
    );
    assert!(
        loaded_after_viewport < total_tiles,
        "Only viewport tiles should be loaded ({} < {}), not the whole bitmap",
        loaded_after_viewport,
        total_tiles
    );

    // --- Correctness: lazy and eager produce identical tile buffers ---
    let eager_tile_buf = eager_renderer
        .get_tile_buffer(1884, 1228, 11, 2, 2, 9)
        .unwrap();
    assert_eq!(
        lazy_tile_buf.to_bytes().unwrap(),
        eager_tile_buf.to_bytes().unwrap(),
        "Lazy and eager should produce identical tile buffer output for the same viewport"
    );

    // 6. Load ALL remaining tiles from the lazy source
    lazy_renderer.ensure_all_tiles_loaded();
    assert_eq!(
        lazy_renderer.peek_latest_bitmap().tiles.len(),
        total_tiles,
        "After ensure_all_tiles_loaded, all {} tiles should be present",
        total_tiles
    );

    // --- Full bitmap equality after loading everything ---
    assert_eq!(
        *lazy_renderer.peek_latest_bitmap(),
        *eager_renderer.peek_latest_bitmap(),
        "After loading all tiles, lazy and eager bitmaps should be identical"
    );
}
