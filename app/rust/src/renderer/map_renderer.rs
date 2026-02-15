use journey_kernel::TileBuffer;

use crate::journey_area_utils;
use crate::journey_bitmap::{JourneyBitmap, Tile};
use crate::journey_data::{self, TileLocation};
use crate::renderer::tile_shader2::TileShader2;
use std::collections::{HashMap, HashSet};

const TILE_ZOOM: i16 = 9;

/// Holds a serialized bitmap blob and a tile index for on-demand decompression.
pub struct LazyTileSource {
    raw_data: Vec<u8>,
    tile_index: HashMap<(u16, u16), TileLocation>,
}

impl LazyTileSource {
    /// Build from a raw serialized bitmap blob (from cache_db).
    /// Only parses tile headers — no tile data is decompressed.
    pub fn from_serialized_bitmap(raw_data: Vec<u8>) -> anyhow::Result<Self> {
        let tile_index = journey_data::parse_tile_index(&raw_data)?;
        Ok(Self {
            raw_data,
            tile_index,
        })
    }

    /// Decompress a single tile on demand.
    pub fn decompress_tile(&self, x: u16, y: u16) -> Option<Tile> {
        let loc = self.tile_index.get(&(x, y))?;
        let tile_data = &self.raw_data[loc.offset..loc.offset + loc.length];
        journey_data::deserialize_tile(tile_data).ok()
    }

    pub fn tile_keys(&self) -> impl Iterator<Item = &(u16, u16)> {
        self.tile_index.keys()
    }
}

pub struct MapRenderer {
    journey_bitmap: JourneyBitmap,
    lazy_source: Option<LazyTileSource>,
    loaded_tiles: HashSet<(u16, u16)>,
    /* for each tile of 512*512 tiles in a JourneyBitmap, use buffered area to record any update */
    tile_area_cache: HashMap<(u16, u16), f64>,
    version: u64,
    current_area: Option<u64>,
}

impl MapRenderer {
    pub fn new(journey_bitmap: JourneyBitmap) -> Self {
        let mut journey_bitmap = journey_bitmap;
        Self::prepare_journey_bitmap_for_rendering(&mut journey_bitmap);
        Self {
            journey_bitmap,
            lazy_source: None,
            loaded_tiles: HashSet::new(),
            tile_area_cache: HashMap::new(),
            version: 0,
            current_area: None,
        }
    }

    fn prepare_journey_bitmap_for_rendering(journey_bitmap: &mut JourneyBitmap) {
        for tile in journey_bitmap.tiles.values_mut() {
            Self::prepare_tiles_for_rendering(tile);
        }
    }

    fn prepare_tiles_for_rendering(_tile: &mut Tile) {
        // just a place holder for future implementation
    }

    pub fn update<F>(&mut self, f: F)
    where
        F: Fn(&mut JourneyBitmap, &mut dyn FnMut((u16, u16))),
    {
        // Collect changed tile positions first
        let mut changed_tiles = Vec::new();
        let mut tile_changed = |tile_pos: (u16, u16)| {
            changed_tiles.push(tile_pos);
        };

        // Apply the update function
        f(&mut self.journey_bitmap, &mut tile_changed);

        // Now prepare tiles for rendering for all changed tiles
        for tile_pos in changed_tiles {
            if let Some(tile) = self.journey_bitmap.tiles.get_mut(&tile_pos) {
                Self::prepare_tiles_for_rendering(tile);
            }
            // Invalidate cache for this tile
            self.tile_area_cache.remove(&tile_pos);
        }

        // TODO: we should improve the cache invalidation rule
        self.reset();
    }

    pub fn replace(&mut self, journey_bitmap: JourneyBitmap) {
        let mut journey_bitmap = journey_bitmap;
        Self::prepare_journey_bitmap_for_rendering(&mut journey_bitmap);
        self.journey_bitmap = journey_bitmap;
        self.lazy_source = None;
        self.loaded_tiles.clear();
        self.tile_area_cache.clear();
        self.reset();
    }

    /// Replace with a lazy tile source for finalized journeys and a small bitmap
    /// containing just the ongoing journey data.
    pub fn replace_lazy(
        &mut self,
        lazy_source: LazyTileSource,
        ongoing_bitmap: JourneyBitmap,
    ) {
        let mut ongoing_bitmap = ongoing_bitmap;
        Self::prepare_journey_bitmap_for_rendering(&mut ongoing_bitmap);
        self.journey_bitmap = ongoing_bitmap;
        self.lazy_source = Some(lazy_source);
        self.loaded_tiles.clear();
        self.tile_area_cache.clear();
        self.reset();
    }

    /// Drop the lazy source and loaded tiles to free memory (e.g. for power saving).
    pub fn drop_lazy_source(&mut self) {
        self.lazy_source = None;
        self.loaded_tiles.clear();
    }

    fn reset(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.current_area = None;
    }

    pub fn get_current_version(&self) -> u64 {
        self.version
    }

    pub fn get_version_string(&self) -> String {
        format!("{:x}", self.version)
    }

    pub fn parse_version_string(version_str: &str) -> Option<u64> {
        // Remove quotes if present
        let cleaned = version_str.trim_matches('"');
        u64::from_str_radix(cleaned, 16).ok()
    }

    pub fn has_changed_since(&self, client_version: Option<&str>) -> Option<String> {
        match client_version {
            Some(v_str) if (Self::parse_version_string(v_str) == Some(self.version)) => None,
            _ => Some(self.get_version_string()),
        }
    }

    // TODO: deprecate this method and merge it with `get_tile_buffer`.
    pub fn get_latest_bitmap_if_changed(
        &self,
        client_version: Option<&str>,
    ) -> Option<(&JourneyBitmap, String)> {
        match client_version {
            Some(v_str) if (Self::parse_version_string(v_str) == Some(self.version)) => None,
            _ => Some((&self.journey_bitmap, self.get_version_string())),
        }
    }

    pub fn peek_latest_bitmap(&self) -> &JourneyBitmap {
        &self.journey_bitmap
    }

    pub fn get_current_area(&mut self) -> u64 {
        // Load all tiles from lazy source before computing area
        self.ensure_all_tiles_loaded();
        *self.current_area.get_or_insert_with(|| {
            journey_area_utils::compute_journey_bitmap_area(
                &self.journey_bitmap,
                Some(&mut self.tile_area_cache),
            )
        })
    }

    /// Ensure a single tile is loaded from the lazy source into the journey_bitmap.
    fn ensure_tile_loaded(&mut self, x: u16, y: u16) {
        if self.loaded_tiles.contains(&(x, y)) {
            return;
        }
        self.loaded_tiles.insert((x, y));

        if let Some(ref lazy) = self.lazy_source {
            if let Some(mut finalized_tile) = lazy.decompress_tile(x, y) {
                Self::prepare_tiles_for_rendering(&mut finalized_tile);
                match self.journey_bitmap.tiles.get_mut(&(x, y)) {
                    Some(existing_tile) => {
                        // Tile already has ongoing journey data — merge finalized into it
                        existing_tile.merge_from(&finalized_tile);
                    }
                    None => {
                        self.journey_bitmap.tiles.insert((x, y), finalized_tile);
                    }
                }
            }
        }
    }

    /// Load all tiles from the lazy source.
    /// Called automatically by `get_current_area`. Also useful when you need
    /// the full bitmap through `peek_latest_bitmap`.
    pub fn ensure_all_tiles_loaded(&mut self) {
        if let Some(ref lazy) = self.lazy_source {
            let keys: Vec<(u16, u16)> = lazy.tile_keys().copied().collect();
            for (x, y) in keys {
                self.ensure_tile_loaded(x, y);
            }
        }
    }

    pub fn get_tile_buffer(
        &mut self,
        x: i64,
        y: i64,
        z: i16,
        width: i64,
        height: i64,
        buffer_size_power: i16,
    ) -> Result<TileBuffer, String> {
        // Pre-load needed tiles from lazy source before rendering
        if self.lazy_source.is_some() {
            let needed = compute_needed_bitmap_tiles(x, y, z, width, height);
            for (tx, ty) in needed {
                self.ensure_tile_loaded(tx, ty);
            }
        }

        tile_buffer_from_journey_bitmap(
            &self.journey_bitmap,
            x,
            y,
            z,
            width,
            height,
            buffer_size_power,
        )
    }
}

/// Compute which bitmap tiles (at zoom 9) are needed for a given viewport.
fn compute_needed_bitmap_tiles(
    x: i64,
    y: i64,
    z: i16,
    width: i64,
    height: i64,
) -> HashSet<(u16, u16)> {
    let mut tiles = HashSet::new();
    let zoom_diff = z - TILE_ZOOM;
    let map_width = 1i64 << 9; // 512

    for dy in 0..height {
        for dx in 0..width {
            let view_x = x + dx;
            let view_y = y + dy;

            if zoom_diff >= 0 {
                // Zoomed in: each view tile maps to one bitmap tile
                let bx = view_x >> zoom_diff;
                let by = view_y >> zoom_diff;
                let bx_wrapped = ((bx % map_width) + map_width) % map_width;
                if by >= 0 && by < map_width {
                    tiles.insert((bx_wrapped as u16, by as u16));
                }
            } else {
                // Zoomed out: each view tile covers multiple bitmap tiles
                let scale = 1i64 << (-zoom_diff);
                let bx_start = view_x * scale;
                let by_start = view_y * scale;
                for bby in by_start..by_start + scale {
                    for bbx in bx_start..bx_start + scale {
                        let wrapped = ((bbx % map_width) + map_width) % map_width;
                        if bby >= 0 && bby < map_width {
                            tiles.insert((wrapped as u16, bby as u16));
                        }
                    }
                }
            }
        }
    }

    tiles
}

/// Create a new TileBuffer from a JourneyBitmap for a range of tiles
fn tile_buffer_from_journey_bitmap(
    journey_bitmap: &JourneyBitmap,
    x: i64,
    y: i64,
    z: i16,
    width: i64,
    height: i64,
    buffer_size_power: i16,
) -> Result<TileBuffer, String> {
    // Validate parameters to prevent overflow and invalid operations
    if width <= 0 || height <= 0 {
        return Err(format!(
            "Invalid dimensions: width={width}, height={height}"
        ));
    }

    if width > 20 || height > 20 {
        return Err(format!(
            "Dimensions too large: width={width}, height={height} (max: 20x20)"
        ));
    }

    if !(0..=25).contains(&z) {
        return Err(format!("Invalid zoom level: {z} (must be 0-25)"));
    }

    if !(6..=11).contains(&buffer_size_power) {
        return Err(format!(
            "Invalid buffer_size_power: {buffer_size_power} (must be 6-11, corresponding to 64-2048 pixel tiles)"
        ));
    }

    // Calculate mercator coordinate cycle length for zoom level z (used for validation and processing)
    let zoom_coefficient = 1i64 << z;

    // Validate coordinate bounds for the given zoom level
    if y < 0 || y >= zoom_coefficient {
        return Err(format!(
            "Invalid y coordinate: {} (must be 0-{})",
            y,
            zoom_coefficient - 1
        ));
    }

    // Create buffer with validated parameters
    let mut buffer = TileBuffer {
        x,
        y,
        z,
        width,
        height,
        buffer_size_power,
        tile_data: vec![Vec::new(); (width * height) as usize],
    };

    // For each tile in the range
    for tile_y in y..(y + height) {
        for tile_x in x..(x + width) {
            // Round off tile_x to ensure it's within mercator coordinate range (0 to 2^z-1)
            let tile_x_rounded =
                ((tile_x % zoom_coefficient) + zoom_coefficient) % zoom_coefficient;

            // Get the pixels using TileShader2
            let pixels = TileShader2::get_pixels_coordinates(
                0,
                0,
                journey_bitmap,
                tile_x_rounded,
                tile_y,
                z,
                buffer_size_power,
            );

            // Convert to tile-relative coordinates and add to buffer
            let idx = buffer.calculate_tile_index(tile_x, tile_y);

            // Bounds check for safety (should never fail with our validation above)
            if idx >= buffer.tile_data.len() {
                return Err(format!(
                    "Index out of bounds: {} >= {}",
                    idx,
                    buffer.tile_data.len()
                ));
            }

            let tile_pixels = &mut buffer.tile_data[idx];

            // Convert from i64 coordinates to u16 coordinates for the TileBuffer
            for (px, py) in pixels {
                if px >= 0
                    && px < (1 << buffer_size_power)
                    && py >= 0
                    && py < (1 << buffer_size_power)
                {
                    // Only add if not already present
                    let pixel = (px as u16, py as u16);
                    tile_pixels.push(pixel);
                }
            }
        }
    }

    Ok(buffer)
}
