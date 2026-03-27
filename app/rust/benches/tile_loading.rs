use criterion::{criterion_group, criterion_main, Criterion};
use memolanes_core::{
    import_data,
    journey_data,
    renderer::map_renderer::MapRenderer,
    utils::lng_lat_to_tile_x_y,
};
use std::time::Duration;

/// Serialize a bitmap to bytes, then benchmark deserialization.
fn bench_bitmap_deserialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("tile_loading");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(15));

    // Load test data (this fully deserializes via FOW import)
    let (bitmap, _warnings) =
        import_data::load_fow_sync_data("./tests/data/fow_3.zip").unwrap();

    // Pre-serialize to get canonical bitmap bytes
    let mut serialized = Vec::new();
    journey_data::serialize_journey_bitmap(&bitmap, &mut serialized).unwrap();
    let serialized_len = serialized.len();

    // Count tiles/blocks for reporting
    let tile_count = bitmap.tiles.len();
    let block_count: usize = bitmap.tiles.values()
        .map(|t| {
            let blocks = t.blocks();
            blocks.iter().filter(|b| b.is_some()).count()
        })
        .sum();

    println!(
        "\n=== Bitmap stats: {} tiles, {} blocks, {} bytes serialized ===",
        tile_count, block_count, serialized_len
    );

    group.bench_function("deserialize_full_bitmap", |b| {
        b.iter(|| {
            std::hint::black_box(
                journey_data::deserialize_journey_bitmap(serialized.as_slice()).unwrap(),
            )
        })
    });

    group.finish();
}

/// Benchmark rendering a specific viewport area via get_tile_buffer.
fn bench_viewport_rendering(c: &mut Criterion) {
    let mut group = c.benchmark_group("viewport_rendering");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(10));

    // Load and serialize to get raw tiles
    let (bitmap, _warnings) =
        import_data::load_fow_sync_data("./tests/data/fow_3.zip").unwrap();
    let mut serialized = Vec::new();
    journey_data::serialize_journey_bitmap(&bitmap, &mut serialized).unwrap();
    // Deserialize to get raw tiles (new behavior)
    let bitmap_raw = journey_data::deserialize_journey_bitmap(serialized.as_slice()).unwrap();

    let mut map_renderer = MapRenderer::new(bitmap_raw);

    // Shenzhen area
    let lng = 114.212470;
    let lat = 22.697006;

    // Zoom 12 - typical street-level view
    let zoom: i16 = 12;
    let (tile_x, tile_y) = lng_lat_to_tile_x_y(lng, lat, zoom as i32);
    let (tile_x, tile_y) = (tile_x as i64, tile_y as i64);

    group.bench_function("get_tile_buffer_z12_2x2", |b| {
        b.iter(|| {
            std::hint::black_box(
                map_renderer
                    .get_tile_buffer(tile_x, tile_y, zoom, 2, 2, 8)
                    .unwrap(),
            )
        })
    });

    // Zoom 5 - continental view
    let zoom5: i16 = 5;
    let (tx5, ty5) = lng_lat_to_tile_x_y(lng, lat, zoom5 as i32);
    let (tx5, ty5) = (tx5 as i64, ty5 as i64);

    group.bench_function("get_tile_buffer_z5_2x2", |b| {
        b.iter(|| {
            std::hint::black_box(
                map_renderer
                    .get_tile_buffer(tx5, ty5, zoom5, 2, 2, 8)
                    .unwrap(),
            )
        })
    });

    group.finish();
}

/// Measure approximate memory usage of a deserialized bitmap (now uses raw tiles).
fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    let (bitmap, _warnings) =
        import_data::load_fow_sync_data("./tests/data/fow_3.zip").unwrap();

    // Pre-serialize
    let mut serialized = Vec::new();
    journey_data::serialize_journey_bitmap(&bitmap, &mut serialized).unwrap();

    // Measure: deserialize (now returns raw tiles) and compute memory footprint
    group.bench_function("deserialize_and_measure_memory", |b| {
        b.iter(|| {
            let bm =
                journey_data::deserialize_journey_bitmap(serialized.as_slice()).unwrap();
            // With raw tiles, each tile stores a Vec<u8> of compressed data.
            // No blocks are allocated until .blocks() is called.
            let tile_count = bm.tiles.len();
            let raw_data_bytes: usize = bm.tiles.values()
                .map(|t| t.raw_data().len())
                .sum();
            let tile_overhead = tile_count * (std::mem::size_of::<Vec<u8>>() + 24);
            let approx_bytes = raw_data_bytes + tile_overhead;
            std::hint::black_box(approx_bytes)
        })
    });

    // Print the memory estimate once
    {
        let bm = journey_data::deserialize_journey_bitmap(serialized.as_slice()).unwrap();
        let tile_count = bm.tiles.len();
        let raw_data_bytes: usize = bm.tiles.values()
            .map(|t| t.raw_data().len())
            .sum();
        let tile_overhead = tile_count * 64; // approximate per-tile struct overhead
        let total = raw_data_bytes + tile_overhead;
        println!(
            "\n=== Memory estimate (raw tiles): {} tiles ===",
            tile_count
        );
        println!(
            "=== Raw data: {:.1}KB, Tile overhead: {:.1}KB ===",
            raw_data_bytes as f64 / 1024.0,
            tile_overhead as f64 / 1024.0
        );
        println!(
            "=== Total approx: {:.1}KB ({} bytes serialized) ===",
            total as f64 / 1024.0,
            serialized.len()
        );
    }

    group.finish();
}

criterion_group!(
    tile_loading_benches,
    bench_bitmap_deserialization,
    bench_viewport_rendering,
    bench_memory_usage,
);
criterion_main!(tile_loading_benches);
