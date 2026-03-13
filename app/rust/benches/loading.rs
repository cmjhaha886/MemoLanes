use criterion::{criterion_group, criterion_main, Criterion};
use memolanes_core::{
    import_data,
    journey_data,
    renderer::map_renderer::MapRenderer,
    utils::lng_lat_to_tile_x_y,
};

fn measure_memory_bytes() -> usize {
    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        #[link(name = "psapi")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                ppsmemCounters: *mut ProcessMemoryCounters,
                cb: u32,
            ) -> i32;
        }
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        unsafe {
            let mut pmc = MaybeUninit::<ProcessMemoryCounters>::zeroed().assume_init();
            pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc,
                pmc.cb,
            );
            pmc.working_set_size
        }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

fn loading_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("loading");
    group.sample_size(10);

    // ---------- Setup ----------

    let (bitmap_large, _) =
        import_data::load_fow_sync_data("./tests/data/fow_3.zip").unwrap();

    let mut blob_large = Vec::new();
    journey_data::serialize_journey_bitmap(&bitmap_large, &mut blob_large).unwrap();

    println!(
        "fow_3: {} tiles, serialized {} bytes ({:.2} MB)",
        bitmap_large.tiles.len(),
        blob_large.len(),
        blob_large.len() as f64 / (1024.0 * 1024.0),
    );

    // Viewport: Shenzhen universiade, zoom 11, 3x3 tiles, 512px
    let lng = 114.212470_f64;
    let lat = 22.697006_f64;
    let z: i16 = 11;
    let (tile_x, tile_y) = lng_lat_to_tile_x_y(lng, lat, z as i32);
    let (tile_x, tile_y) = (tile_x as i64, tile_y as i64);
    let width: i64 = 3;
    let height: i64 = 3;
    let buffer_size_power: i16 = 9; // 512px

    // ---------- Memory measurement ----------
    // Measure lazy first (smaller), then eager, so the working set grows monotonically.

    let mem_baseline = measure_memory_bytes();

    let lazy_bitmap =
        journey_data::deserialize_journey_bitmap_lazy(blob_large.clone()).unwrap();
    let mem_after_lazy = measure_memory_bytes();
    let lazy_mem = mem_after_lazy.saturating_sub(mem_baseline);

    println!(
        "Memory: lazy bitmap: ~{:.2} MB (raw blob {} + tile index)",
        lazy_mem as f64 / (1024.0 * 1024.0),
        blob_large.len(),
    );
    drop(lazy_bitmap);

    // For eager, measure in a fresh baseline since working set doesn't shrink on drop.
    let mem_before_eager = measure_memory_bytes();
    let eager_bitmap =
        journey_data::deserialize_journey_bitmap(blob_large.as_slice()).unwrap();
    let mem_after_eager = measure_memory_bytes();
    let eager_mem = mem_after_eager.saturating_sub(mem_before_eager);

    println!(
        "Memory: eager bitmap: ~{:.2} MB",
        eager_mem as f64 / (1024.0 * 1024.0),
    );
    drop(eager_bitmap);

    // ---------- 1. Initial load time ----------

    group.bench_function("init_eager: fow_3", |b| {
        b.iter(|| {
            std::hint::black_box(
                journey_data::deserialize_journey_bitmap(blob_large.as_slice()).unwrap(),
            )
        })
    });

    group.bench_function("init_lazy: fow_3", |b| {
        b.iter(|| {
            std::hint::black_box(
                journey_data::deserialize_journey_bitmap_lazy(blob_large.clone()).unwrap(),
            )
        })
    });

    // ---------- 2. Render viewport (bitmap already initialized) ----------

    // Eager: pre-built MapRenderer, measure only rendering
    let eager_bitmap =
        journey_data::deserialize_journey_bitmap(blob_large.as_slice()).unwrap();
    let mut eager_renderer = MapRenderer::new(eager_bitmap);

    group.bench_function("render_viewport (eager, pre-loaded)", |b| {
        b.iter(|| {
            std::hint::black_box(
                eager_renderer
                    .get_tile_buffer(tile_x, tile_y, z, width, height, buffer_size_power)
                    .unwrap(),
            )
        })
    });

    // Lazy: re-create lazy bitmap each iteration (init is fast),
    // get_tile_buffer ensures only the viewport tiles on demand.
    group.bench_function("render_viewport (lazy, on-demand tile loading)", |b| {
        b.iter(|| {
            let bitmap =
                journey_data::deserialize_journey_bitmap_lazy(blob_large.clone()).unwrap();
            let mut renderer = MapRenderer::new(bitmap);
            std::hint::black_box(
                renderer
                    .get_tile_buffer(tile_x, tile_y, z, width, height, buffer_size_power)
                    .unwrap(),
            )
        })
    });

    group.finish();
}

criterion_group!(loading_benches, loading_benchmarks);
criterion_main!(loading_benches);
