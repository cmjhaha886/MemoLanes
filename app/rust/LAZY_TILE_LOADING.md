# Lazy Tile Loading for Main Map

## Problem

When the user opens the map, `init_main_map()` deserializes the **entire** cached bitmap — decompressing every tile's zstd data and building the full in-memory `JourneyBitmap`. For users with extensive journey history (many tiles across many regions), this causes a noticeable startup delay.

However, the frontend only shows a small viewport at a time and requests tiles by coordinates `(x, y, z, width, height)`. Most tiles are off-screen and don't need to be loaded immediately.

## Original Logic

### Data Flow

```
init_main_map()
│
├─► cache_db.get_full_journey_cache(layer_kind)
│     │
│     └─► SELECT data FROM journey_cache__full
│         └─► JourneyData::deserialize(blob)
│               │
│               └─► For EVERY tile in blob:
│                     read (x, y, size)
│                     zstd decompress tile data    ◄── SLOW (repeated N times)
│                     parse all 128×128 blocks
│                     insert into HashMap
│               └─► Returns full JourneyBitmap
│
├─► If ongoing journey exists:
│     rasterize GPS points into bitmap (merge)
│
└─► map_renderer.replace(full_bitmap)              ◄── Map ready (after full decompression)
```

### Tile Request (after init)

```
Frontend: GET /tile?x=...&y=...&z=...&w=...&h=...
│
└─► MapRenderer.get_tile_buffer(&self, x, y, z, w, h)
      └─► TileShader2 reads from JourneyBitmap.tiles HashMap
          (all tiles already in memory)
      └─► Returns TileBuffer with pixel coordinates
```

### Problem Summary

- **All tiles decompressed at startup** even though user only sees ~4-6 tiles
- Decompression is CPU-bound (zstd) and scales with total journey coverage
- Users who have traveled extensively wait longest

## New Logic (Lazy Tile Loading)

### Data Flow

```
init_main_map()                                     ◄── FAST (no decompression)
│
├─► cache_db.get_full_journey_cache_raw(layer_kind)
│     │
│     └─► SELECT data FROM journey_cache__full
│         └─► Returns raw blob bytes (no deserialization)
│
├─► LazyTileSource::from_serialized_bitmap(raw_blob)
│     │
│     └─► parse_tile_index(raw_blob)
│           │
│           └─► For each tile: read (x, y, size) headers ONLY
│               Record byte offset + length
│               SKIP compressed data (no decompression)
│           └─► Returns HashMap<(u16,u16), TileLocation>
│
├─► If ongoing journey exists:
│     rasterize GPS points into small JourneyBitmap
│     (only tiles touched by current recording)
│
└─► map_renderer.replace_lazy(lazy_source, ongoing_bitmap)
      └─► Stores LazyTileSource + small ongoing bitmap
          Map ready immediately
```

### Tile Request (on-demand loading)

```
Frontend: GET /tile?x=...&y=...&z=...&w=...&h=...
│
└─► MapRenderer.get_tile_buffer(&mut self, x, y, z, w, h)
      │
      ├─► compute_needed_bitmap_tiles(x, y, z, w, h)
      │     Converts viewport tile coords → bitmap tile coords (zoom 9)
      │     Returns HashSet<(u16, u16)> of needed tiles
      │
      ├─► For each needed tile (tx, ty):
      │     ensure_tile_loaded(tx, ty)
      │       │
      │       ├─ Already in loaded_tiles set? → Skip
      │       │
      │       └─ Not loaded yet:
      │            ├─ Decompress from LazyTileSource (single tile zstd)
      │            ├─ If ongoing bitmap already has this tile:
      │            │    merge finalized data INTO ongoing (bitwise OR)
      │            └─ Else: insert finalized tile into bitmap
      │            Mark as loaded
      │
      └─► TileShader2 reads from JourneyBitmap
          (now contains needed tiles)
      └─► Returns TileBuffer
```

## Key Components

### Serialization Format (unchanged)

```
[Magic: 0xB0]
[tile_count: varint]
[tile_0: [x: u16][y: u16][size: varint][zstd_compressed_data]]
[tile_1: [x: u16][y: u16][size: varint][zstd_compressed_data]]
...
```

The existing format already has per-tile size headers, which allows skipping individual tiles without decompression.

### LazyTileSource (`map_renderer.rs`)

```
LazyTileSource {
    raw_data: Vec<u8>,                               // full cache blob (kept in memory)
    tile_index: HashMap<(u16,u16), TileLocation>,    // tile (x,y) → byte offset + length
}
```

- `from_serialized_bitmap(blob)` — parses headers only, O(N) where N = tile count
- `decompress_tile(x, y)` — decompresses a single tile on demand
- `tile_keys()` — iterate all available tile coordinates

### MapRenderer Changes

```
MapRenderer {
    journey_bitmap: JourneyBitmap,       // partially loaded (ongoing + loaded tiles)
    lazy_source: Option<LazyTileSource>, // raw data for unloaded tiles
    loaded_tiles: HashSet<(u16, u16)>,   // tracks which tiles are already decompressed
    ...
}
```

Key methods:
- `replace_lazy(lazy_source, ongoing_bitmap)` — fast init with lazy source
- `ensure_tile_loaded(x, y)` — decompress + merge a single tile on demand
- `ensure_all_tiles_loaded()` — force-load all tiles (used by area calculation)
- `get_tile_buffer(&mut self, ...)` — changed from `&self` to `&mut self` for lazy loading

### Tile::merge_from (`journey_bitmap.rs`)

When a lazy tile is loaded, it may need to merge with existing ongoing journey data:

```
For each block in other_tile:
    if self has block at same position:
        bitwise OR (merge_with)
    else:
        clone other's block into self
```

## Cache Miss Fallback

On first run (no cache exists), the system falls back to the original eager path:

```
get_latest_lazy()
│
├─► Try get_full_journey_cache_raw() → None (no cache)
│
└─► Fall back to get_latest() (eager: full deserialization)
      ├─► Computes full bitmap
      ├─► Populates cache for next time
      └─► Returns (None, full_bitmap)
            └─► MapRenderer.replace(full_bitmap)  // eager mode
```

Next time the map is opened, the cache exists and the lazy path is used.

## Files Modified

| File | Changes |
|------|---------|
| `src/journey_data.rs` | Added `TileLocation`, `parse_tile_index()`, made `deserialize_tile` pub |
| `src/journey_bitmap.rs` | Added `Tile::merge_from()` |
| `src/renderer/map_renderer.rs` | Added `LazyTileSource`, lazy fields, `ensure_tile_loaded`, `compute_needed_bitmap_tiles`, `get_tile_buffer` → `&mut self` |
| `src/renderer/internal_server.rs` | `handle()` and `handle_tile_range_query()` → `&mut MapRenderer` |
| `src/cache_db.rs` | Added `get_full_journey_cache_raw()` |
| `src/merged_journey_builder.rs` | Added `get_latest_lazy()` |
| `src/storage.rs` | Added `get_latest_bitmap_lazy()` |
| `src/api/api.rs` | Updated `reload_main_map_bitmap()` to use lazy path |

## Test: Lazy Loading Correctness & Performance

The test `lazy_loading_correctness_and_performance` (in `tests/map_renderer.rs`) validates both the correctness and performance of the lazy tile loading path. It works as follows:

1. **Build a multi-region bitmap** — draws lines across 15 cities worldwide (Sydney, Tokyo, London, New York, Rio, Beijing, Delhi, Paris, Moscow, Los Angeles, Singapore, São Paulo, Auckland, Istanbul, Bangkok), producing 20 distinct bitmap tiles.
2. **Serialize the bitmap** (33 583 bytes) to simulate the on-disk cache format.
3. **Eager path** — fully deserializes (zstd-decompresses every tile) the blob, measuring init time.
4. **Lazy path** — calls `LazyTileSource::from_serialized_bitmap`, which only parses tile headers (coordinates + byte offsets) without decompressing any tile data.
5. **Assertions:**
   - Lazy init is faster than eager init (header-only vs full decompression).
   - No tiles are loaded in the lazy renderer's bitmap immediately after init.
   - Requesting a single viewport (Sydney, zoom 11) loads only the tiles needed (1 out of 20).
   - The tile buffer produced by the lazy path is byte-identical to the eager path for the same viewport.
   - After `ensure_all_tiles_loaded()`, the lazy bitmap equals the eager bitmap exactly.

### Test Result

```
running 1 test
Total tiles in bitmap: 20
Serialized bitmap size: 33583 bytes
Eager init (full deserialization): 18.061ms
Lazy init (header parsing only):   69.8µs
Tiles loaded after Sydney viewport request: 1 / 20
test lazy_loading_correctness_and_performance ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.10s
```

Lazy init is **~259× faster** than eager init (69.8 µs vs 18.061 ms), and only 1 of 20 tiles is decompressed when the user views the Sydney area — confirming that the lazy path avoids unnecessary work at startup.

## Edge Cases

- **Cache miss (first run):** Falls back to eager loading. Cache gets populated for next time.
- **Ongoing journey updates:** Writes to `journey_bitmap` directly. When lazy tiles load later, they merge correctly via bitwise OR.
- **Area calculation:** Triggers `ensure_all_tiles_loaded()` since it needs the full bitmap. Acceptable because area is shown in stats, not during map init.
- **Power saving mode:** Calls `drop_lazy_source()` to release the raw blob memory.
- **Cache invalidated (new journey finalized):** Rebuilds `LazyTileSource` from updated cache, resets `loaded_tiles`.
