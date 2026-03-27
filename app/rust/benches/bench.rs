use criterion::{criterion_group, criterion_main, Criterion};
use memolanes_core::{
    gps_processor::SegmentGapRule, import_data, journey_area_utils, journey_bitmap::JourneyBitmap,
};

fn deserialize_all_tiles(
    bitmap: &JourneyBitmap,
) -> std::collections::HashMap<(u16, u16), Box<memolanes_core::journey_bitmap::TileBlocks>> {
    bitmap
        .tiles
        .iter()
        .map(|(pos, tile)| (*pos, tile.blocks()))
        .collect()
}

fn journey_area_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("area_calculation");
    group.sample_size(10);

    group.bench_function("compute_journey_bitmap_area: simple", |b| {
        let (bitmap_import, _warnings) =
            import_data::load_fow_sync_data("./tests/data/fow_1.zip").unwrap();
        b.iter(|| {
            let tiles = deserialize_all_tiles(&bitmap_import);
            std::hint::black_box(journey_area_utils::compute_journey_bitmap_area_from_tiles(
                &tiles, None,
            ))
        })
    });

    group.bench_function(
        "compute_journey_bitmap_area: nelson_to_wharariki_beach",
        |b| {
            let (raw_data, _preprocessor) =
                import_data::load_gpx("./tests/data/nelson_to_wharariki_beach.gpx").unwrap();

            let journey_vector =
                import_data::journey_vector_from_raw_data_with_gps_preprocessor(&raw_data, None)
                    .unwrap();
            let mut journey_bitmap = JourneyBitmap::new();
            journey_bitmap.merge_vector(&journey_vector);
            b.iter(|| {
                let tiles = deserialize_all_tiles(&journey_bitmap);
                std::hint::black_box(journey_area_utils::compute_journey_bitmap_area_from_tiles(
                    &tiles, None,
                ))
            })
        },
    );

    group.finish();
}

fn journey_bitmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("journey_bitmap");
    group.sample_size(40);

    group.bench_function("merge_vector", |b| {
        let load_journey_vector = |name| {
            let filename = format!("./tests/data/{name}.gpx");
            let (raw_data, _preprocessor) = import_data::load_gpx(&filename).unwrap();
            import_data::journey_vector_from_raw_data_with_gps_preprocessor(
                &raw_data,
                Some(SegmentGapRule::Default),
            )
            .unwrap()
        };

        let nelson_to_wharariki_beach = load_journey_vector("nelson_to_wharariki_beach");
        let heihe = load_journey_vector("raw_gps_heihe");

        b.iter(|| {
            let mut journey_bitmap = JourneyBitmap::new();
            std::hint::black_box(journey_bitmap.merge_vector(&nelson_to_wharariki_beach));
            std::hint::black_box(journey_bitmap.merge_vector(&heihe));
            journey_bitmap
        })
    });

    group.finish();
}

criterion_group!(benches, journey_area_calculation, journey_bitmap);
criterion_main!(benches);
