/* We store journey one by one, but for a lot of use cases such as rendering, we
need to merge all journeys into one `journey_bitmap`. Relavent functionailties is
implemented here.
*/
use crate::{
    cache_db::{CacheDb, LayerKind},
    journey_bitmap::JourneyBitmap,
    journey_data::JourneyData,
    journey_header::JourneyKind,
    journey_vector::JourneyVector,
    main_db::{self, MainDb},
    renderer::map_renderer::LazyTileSource,
};
use anyhow::{Context, Result};
use auto_context::auto_context;
use chrono::NaiveDate;

pub fn add_journey_vector_to_journey_bitmap(
    journey_bitmap: &mut JourneyBitmap,
    journey_vector: &JourneyVector,
) {
    for track_segmant in &journey_vector.track_segments {
        for (i, point) in track_segmant.track_points.iter().enumerate() {
            let prev_idx = i.saturating_sub(1);
            let prev = &track_segmant.track_points[prev_idx];
            journey_bitmap.add_line(
                prev.longitude,
                prev.latitude,
                point.longitude,
                point.latitude,
            );
        }
    }
}

// TODO: This is going to be very slow.
// Returns a journey bitmap for the journey kind
#[auto_context]
fn get_range_internal(
    txn: &main_db::Txn,
    from_date_inclusive: Option<NaiveDate>,
    to_date_inclusive: Option<NaiveDate>,
    kind: Option<&JourneyKind>, // TODO: depending on the future design, we might not want this to be optional.
) -> Result<JourneyBitmap> {
    let mut journey_map = JourneyBitmap::new();

    for journey_header in txn.query_journeys(from_date_inclusive, to_date_inclusive)? {
        // TODO: We should just do the filtering in the sql query, instead of here.
        let should_include = match kind {
            None => true,
            Some(kind) => *kind == journey_header.journey_kind,
        };

        if should_include {
            let journey_data = txn.get_journey_data(&journey_header.id)?;
            match journey_data {
                JourneyData::Bitmap(bitmap) => journey_map.merge(bitmap),
                JourneyData::Vector(vector) => {
                    add_journey_vector_to_journey_bitmap(&mut journey_map, &vector);
                }
            }
        }
    }

    Ok(journey_map)
}

// for time machine
#[auto_context]
pub fn get_range(
    txn: &mut main_db::Txn,
    from_date_inclusive: NaiveDate,
    to_date_inclusive: NaiveDate,
    kind: Option<&JourneyKind>,
) -> Result<JourneyBitmap> {
    get_range_internal(
        txn,
        Some(from_date_inclusive),
        Some(to_date_inclusive),
        kind,
    )
}

#[auto_context]
fn get_all_finalized_journeys(
    main_db_txn: &main_db::Txn,
    cache_db: &CacheDb,
    layer_kind: &LayerKind,
) -> Result<JourneyBitmap> {
    cache_db.get_full_journey_cache_or_compute(layer_kind, || match layer_kind {
        LayerKind::All => {
            let mut default_bitmap = get_all_finalized_journeys(
                main_db_txn,
                cache_db,
                &LayerKind::JourneyKind(JourneyKind::DefaultKind),
            )?;
            let flight_bitmap = get_all_finalized_journeys(
                main_db_txn,
                cache_db,
                &LayerKind::JourneyKind(JourneyKind::Flight),
            )?;
            default_bitmap.merge(flight_bitmap);
            Ok(default_bitmap)
        }
        LayerKind::JourneyKind(kind) => get_range_internal(main_db_txn, None, None, Some(kind)),
    })
}

// main map
#[auto_context]
pub fn get_latest(
    main_db: &mut MainDb,
    cache_db: &CacheDb,
    layer_kind: &Option<LayerKind>,
    include_ongoing: bool,
) -> Result<JourneyBitmap> {
    main_db.with_txn(|txn| {
        let mut journey_bitmap = match layer_kind {
            Some(layer_kind) => get_all_finalized_journeys(txn, cache_db, layer_kind)?,
            None => JourneyBitmap::new(),
        };

        if include_ongoing {
            match txn.get_ongoing_journey(None)? {
                None => (),
                Some(journey_vector) => {
                    add_journey_vector_to_journey_bitmap(&mut journey_bitmap, &journey_vector)
                }
            }
        }

        // NOTE: Calling to `main_db.with_txn` directly without going through
        // `storage` is fine here because we are not modifying main db here.
        // But just to make sure:
        assert_eq!(txn.action, None);
        Ok(journey_bitmap)
    })
}

/// Lazy variant of `get_latest` for the main map.
///
/// - **Cache hit:** Returns `(Some(LazyTileSource), ongoing_bitmap)`.
///   The LazyTileSource holds the compressed finalized data and decompresses
///   tiles on demand. The ongoing_bitmap is a small bitmap with just the
///   current recording (if any).
///
/// - **Cache miss:** Falls back to eager computation via `get_latest`, populates
///   the cache, and returns `(None, full_bitmap)`.
#[auto_context]
pub fn get_latest_lazy(
    main_db: &mut MainDb,
    cache_db: &CacheDb,
    layer_kind: &Option<LayerKind>,
    include_ongoing: bool,
) -> Result<(Option<LazyTileSource>, JourneyBitmap)> {
    // Try to get the raw cache blob for the effective layer kind
    let raw_blob = match layer_kind {
        Some(lk) => {
            // For LayerKind::All, we need the "All" cache specifically
            cache_db.get_full_journey_cache_raw(lk)?
        }
        None => None,
    };

    match raw_blob {
        Some(data) => {
            // Cache hit — build lazy source without decompressing any tiles
            let lazy_source = LazyTileSource::from_serialized_bitmap(data)?;

            // Build the (small) ongoing journey bitmap
            let mut ongoing_bitmap = JourneyBitmap::new();
            if include_ongoing {
                main_db.with_txn(|txn| {
                    if let Some(journey_vector) = txn.get_ongoing_journey(None)? {
                        add_journey_vector_to_journey_bitmap(
                            &mut ongoing_bitmap,
                            &journey_vector,
                        );
                    }
                    assert_eq!(txn.action, None);
                    Ok(())
                })?;
            }

            Ok((Some(lazy_source), ongoing_bitmap))
        }
        None => {
            // Cache miss — fall back to full eager loading (also populates cache)
            let full_bitmap = get_latest(main_db, cache_db, layer_kind, include_ongoing)?;
            Ok((None, full_bitmap))
        }
    }
}
