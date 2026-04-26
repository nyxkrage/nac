//! Conway's Game of Life implementation for TUI background animation.
//!
//! This module provides a cellular automaton simulation using Braille patterns
//! for rendering. It includes various seed patterns (gliders, spaceships, etc.)
//! and automatic pattern injection when activity becomes low.

use ratatui::text::{Line, Span};

/// Width scale factor for positioning patterns in the life field.
pub const LIFE_LAYOUT_WIDTH_SCALE: isize = 5;

/// A predefined seed pattern for the Game of Life.
#[derive(Clone, Copy, Debug)]
pub struct SeedPattern {
    /// Width of the pattern in cells.
    pub width: usize,
    /// Height of the pattern in cells.
    pub height: usize,
    /// Coordinates of live cells within the pattern bounds.
    pub cells: &'static [(usize, usize)],
}

/// Placement configuration for injecting a pattern into the life field.
#[derive(Clone, Copy, Debug)]
pub struct PatternPlacement {
    /// The pattern to place.
    pub pattern: SeedPattern,
    /// Horizontal offset from center (scaled by LIFE_LAYOUT_WIDTH_SCALE).
    pub dx: isize,
    /// Vertical offset from center.
    pub dy: isize,
    /// Rotation in 90-degree increments (0-3).
    pub rotation: u8,
    /// Whether to flip horizontally after rotation.
    pub flip: bool,
}

/// The glider pattern - a small spaceship that moves diagonally.
pub const GLIDER_PATTERN: SeedPattern = SeedPattern {
    width: 3,
    height: 3,
    cells: &[(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)],
};

/// The R-pentomino pattern - a small methuselah that evolves for many generations.
pub const R_PENTOMINO_PATTERN: SeedPattern = SeedPattern {
    width: 3,
    height: 3,
    cells: &[(1, 0), (2, 0), (0, 1), (1, 1), (1, 2)],
};

/// The acorn pattern - a methuselah that evolves for 5206 generations.
pub const ACORN_PATTERN: SeedPattern = SeedPattern {
    width: 7,
    height: 3,
    cells: &[(1, 0), (3, 1), (0, 2), (1, 2), (4, 2), (5, 2), (6, 2)],
};

/// The lightweight spaceship (LWSS) pattern - moves orthogonally.
pub const LWSS_PATTERN: SeedPattern = SeedPattern {
    width: 5,
    height: 4,
    cells: &[
        (1, 0),
        (2, 0),
        (3, 0),
        (4, 0),
        (0, 1),
        (4, 1),
        (4, 2),
        (0, 3),
        (3, 3),
    ],
};

/// Collection of pattern layouts for automatic injection during low activity.
pub const LIFE_INJECTION_LAYOUTS: &[&[PatternPlacement]] = &[
    // Layout 0: Four gliders at corners with acorn and R-pentominos
    &[
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: -14,
            dy: -8,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: 14,
            dy: -8,
            rotation: 1,
            flip: false,
        },
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: -14,
            dy: 8,
            rotation: 3,
            flip: false,
        },
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: 14,
            dy: 8,
            rotation: 2,
            flip: false,
        },
        PatternPlacement {
            pattern: ACORN_PATTERN,
            dx: 0,
            dy: -1,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: -5,
            dy: 4,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: 5,
            dy: 4,
            rotation: 2,
            flip: true,
        },
    ],
    // Layout 1: Two LWSS spaceships with acorns and gliders
    &[
        PatternPlacement {
            pattern: LWSS_PATTERN,
            dx: -18,
            dy: -6,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: LWSS_PATTERN,
            dx: 18,
            dy: 6,
            rotation: 2,
            flip: false,
        },
        PatternPlacement {
            pattern: ACORN_PATTERN,
            dx: -8,
            dy: 0,
            rotation: 1,
            flip: false,
        },
        PatternPlacement {
            pattern: ACORN_PATTERN,
            dx: 8,
            dy: 0,
            rotation: 3,
            flip: true,
        },
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: 0,
            dy: -12,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: GLIDER_PATTERN,
            dx: 0,
            dy: 12,
            rotation: 2,
            flip: false,
        },
    ],
    // Layout 2: Four R-pentominos with two LWSS
    &[
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: -10,
            dy: -4,
            rotation: 0,
            flip: false,
        },
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: 10,
            dy: -4,
            rotation: 1,
            flip: false,
        },
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: -10,
            dy: 4,
            rotation: 3,
            flip: true,
        },
        PatternPlacement {
            pattern: R_PENTOMINO_PATTERN,
            dx: 10,
            dy: 4,
            rotation: 2,
            flip: true,
        },
        PatternPlacement {
            pattern: LWSS_PATTERN,
            dx: 0,
            dy: -10,
            rotation: 1,
            flip: false,
        },
        PatternPlacement {
            pattern: LWSS_PATTERN,
            dx: 0,
            dy: 10,
            rotation: 3,
            flip: false,
        },
    ],
];

/// The Game of Life field state.
#[derive(Debug, Clone, Default)]
pub struct LifeField {
    /// Width of the field in cells.
    width: usize,
    /// Height of the field in cells.
    height: usize,
    /// Cell states - true for alive, false for dead.
    cells: Vec<bool>,
    /// Counter for consecutive ticks with low activity.
    low_activity_ticks: usize,
    /// Current phase for pattern injection rotation.
    injection_phase: usize,
}

impl LifeField {
    /// Ensures the field has the specified dimensions, reseeding if changed.
    ///
    /// # Arguments
    /// * `width` - Desired width in cells
    /// * `height` - Desired height in cells
    pub fn ensure_size(&mut self, width: usize, height: usize) {
        let width = width.max(2);
        let height = height.max(4);
        if self.width == width && self.height == height && !self.cells.is_empty() {
            return;
        }

        self.width = width;
        self.height = height;
        self.cells = seed_life_cells(width, height);
        self.low_activity_ticks = 0;
        self.injection_phase = 0;
    }

    /// Advances the simulation by one generation using Conway's rules.
    ///
    /// Rules:
    /// - Live cell with 2-3 neighbors survives
    /// - Dead cell with 3 or 6 neighbors becomes alive (highlife variant for 6)
    /// - All other cells die or stay dead
    ///
    /// When activity becomes low, new patterns are automatically injected.
    pub fn step(&mut self) {
        if self.width == 0 || self.height == 0 || self.cells.is_empty() {
            return;
        }

        let mut next = vec![false; self.cells.len()];
        let mut alive = 0usize;
        let mut changed = 0usize;
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = self.index(x, y);
                let neighbors = self.live_neighbor_count(x, y);
                let next_alive =
                    matches!((self.cells[idx], neighbors), (true, 2 | 3) | (false, 3 | 6));
                next[idx] = next_alive;
                alive += usize::from(next_alive);
                changed += usize::from(self.cells[idx] != next_alive);
            }
        }

        self.cells = if alive == 0 {
            self.low_activity_ticks = 0;
            self.injection_phase = 0;
            seed_life_cells(self.width, self.height)
        } else {
            let low_activity_threshold = (self.width * self.height / 384).max(6);
            if changed <= low_activity_threshold {
                self.low_activity_ticks = self.low_activity_ticks.saturating_add(1);
            } else {
                self.low_activity_ticks = 0;
            }

            if self.low_activity_ticks >= 16 {
                inject_showcase_layout(&mut next, self.width, self.height, self.injection_phase);
                self.injection_phase =
                    (self.injection_phase + 1) % LIFE_INJECTION_LAYOUTS.len().max(1);
                self.low_activity_ticks = 4;
            }

            next
        };
    }

    /// Renders the field as a vector of ratatui Lines using Braille characters.
    ///
    /// Each Braille character represents a 2x4 block of cells, allowing for
    /// compact display of the life field.
    ///
    /// # Arguments
    /// * `char_width` - Width in characters (each char is 2 cells wide)
    /// * `char_height` - Height in characters (each char is 4 cells tall)
    ///
    /// # Returns
    /// Vector of Lines representing the rendered field
    pub fn render_lines(&self, char_width: usize, char_height: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(char_height);
        for char_y in 0..char_height {
            let mut text = String::with_capacity(char_width);
            for char_x in 0..char_width {
                let dot_x = char_x * 2;
                let dot_y = char_y * 4;
                text.push(self.braille_char(dot_x, dot_y));
            }
            lines.push(Line::from(Span::raw(text)));
        }
        lines
    }

    /// Generates a Braille character representing a 2x4 cell block.
    ///
    /// Braille dots are mapped to cell positions:
    /// - Dots 1-4 map to left column (top to bottom)
    /// - Dots 5-8 map to right column (top to bottom)
    ///
    /// # Arguments
    /// * `dot_x` - X coordinate of the left cell in the block
    /// * `dot_y` - Y coordinate of the top cell in the block
    ///
    /// # Returns
    /// A Braille Unicode character (U+2800 to U+28FF)
    pub fn braille_char(&self, dot_x: usize, dot_y: usize) -> char {
        let mut bits = 0u32;
        for local_y in 0..4 {
            for local_x in 0..2 {
                let x = dot_x + local_x;
                let y = dot_y + local_y;
                if x < self.width && y < self.height && self.cells[self.index(x, y)] {
                    bits |= match (local_x, local_y) {
                        (0, 0) => 0x01,
                        (0, 1) => 0x02,
                        (0, 2) => 0x04,
                        (0, 3) => 0x40,
                        (1, 0) => 0x08,
                        (1, 1) => 0x10,
                        (1, 2) => 0x20,
                        (1, 3) => 0x80,
                        _ => 0,
                    };
                }
            }
        }
        char::from_u32(0x2800 + bits).unwrap_or(' ')
    }

    /// Counts the number of live neighbors for a cell.
    ///
    /// Uses toroidal wrapping - edges connect to the opposite side.
    ///
    /// # Arguments
    /// * `x` - X coordinate of the cell
    /// * `y` - Y coordinate of the cell
    ///
    /// # Returns
    /// Count of live neighbors (0-8)
    pub fn live_neighbor_count(&self, x: usize, y: usize) -> u8 {
        let mut count = 0u8;
        for dy in [-1isize, 0, 1] {
            for dx in [-1isize, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = wrap_index(x, dx, self.width);
                let ny = wrap_index(y, dy, self.height);
                if self.cells[self.index(nx, ny)] {
                    count += 1;
                }
            }
        }
        count
    }

    /// Converts 2D coordinates to a 1D array index.
    ///
    /// # Arguments
    /// * `x` - X coordinate
    /// * `y` - Y coordinate
    ///
    /// # Returns
    /// Index into the cells vector
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }
}

/// Creates a new cell vector with seed patterns injected.
///
/// # Arguments
/// * `width` - Field width in cells
/// * `height` - Field height in cells
///
/// # Returns
/// Vector of cell states initialized with patterns
pub fn seed_life_cells(width: usize, height: usize) -> Vec<bool> {
    let mut cells = vec![false; width.saturating_mul(height)];
    if width == 0 || height == 0 {
        return cells;
    }

    inject_showcase_layout(&mut cells, width, height, 0);

    cells
}

/// Injects a showcase layout of patterns into the cell field.
///
/// # Arguments
/// * `cells` - Mutable slice of cell states
/// * `width` - Field width
/// * `height` - Field height
/// * `phase` - Which layout phase to inject (cycles through LIFE_INJECTION_LAYOUTS)
pub fn inject_showcase_layout(cells: &mut [bool], width: usize, height: usize, phase: usize) {
    if width == 0 || height == 0 || cells.is_empty() {
        return;
    }

    let layouts_len = LIFE_INJECTION_LAYOUTS.len();
    if layouts_len == 0 {
        return;
    }

    for placement in LIFE_INJECTION_LAYOUTS[phase % layouts_len] {
        inject_pattern(cells, width, height, *placement);
    }
}

/// Injects a single pattern into the cell field at a specified placement.
///
/// # Arguments
/// * `cells` - Mutable slice of cell states
/// * `width` - Field width
/// * `height` - Field height
/// * `placement` - Pattern placement configuration
pub fn inject_pattern(cells: &mut [bool], width: usize, height: usize, placement: PatternPlacement) {
    if width == 0 || height == 0 {
        return;
    }

    let center_x = (width / 2) as isize;
    let center_y = (height / 2) as isize;
    let anchor_x = center_x + placement.dx.saturating_mul(LIFE_LAYOUT_WIDTH_SCALE);
    let anchor_y = center_y + placement.dy;
    let (placed_width, placed_height) = rotated_dimensions(
        placement.pattern.width,
        placement.pattern.height,
        placement.rotation,
    );
    let origin_x = anchor_x - placed_width as isize / 2;
    let origin_y = anchor_y - placed_height as isize / 2;

    for &(x, y) in placement.pattern.cells {
        let (mut px, py) = rotate_cell(
            x,
            y,
            placement.pattern.width,
            placement.pattern.height,
            placement.rotation,
        );
        if placement.flip {
            px = placed_width.saturating_sub(1).saturating_sub(px);
        }
        let world_x = wrap_index_signed(origin_x + px as isize, width);
        let world_y = wrap_index_signed(origin_y + py as isize, height);
        cells[world_y * width + world_x] = true;
    }
}

/// Wraps an index with a delta, handling toroidal boundaries.
///
/// # Arguments
/// * `index` - Starting index
/// * `delta` - Offset to apply (can be negative)
/// * `len` - Length of the dimension
///
/// # Returns
/// Wrapped index within [0, len)
pub fn wrap_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    ((index as isize + delta).rem_euclid(len as isize)) as usize
}

/// Wraps a signed index, handling toroidal boundaries.
///
/// # Arguments
/// * `index` - Signed index (can be negative)
/// * `len` - Length of the dimension
///
/// # Returns
/// Wrapped index within [0, len)
pub fn wrap_index_signed(index: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.rem_euclid(len as isize) as usize
}

/// Calculates dimensions after rotation.
///
/// # Arguments
/// * `width` - Original width
/// * `height` - Original height
/// * `rotation` - Rotation in 90-degree increments (0-3)
///
/// # Returns
/// (width, height) after rotation
pub fn rotated_dimensions(width: usize, height: usize, rotation: u8) -> (usize, usize) {
    if rotation % 2 == 0 {
        (width, height)
    } else {
        (height, width)
    }
}

/// Rotates a cell coordinate within a pattern.
///
/// # Arguments
/// * `x` - Original X coordinate
/// * `y` - Original Y coordinate
/// * `width` - Pattern width
/// * `height` - Pattern height
/// * `rotation` - Rotation in 90-degree increments (0-3)
///
/// # Returns
/// Rotated (x, y) coordinates
pub fn rotate_cell(x: usize, y: usize, width: usize, height: usize, rotation: u8) -> (usize, usize) {
    match rotation % 4 {
        0 => (x, y),
        1 => (height.saturating_sub(1).saturating_sub(y), x),
        2 => (
            width.saturating_sub(1).saturating_sub(x),
            height.saturating_sub(1).saturating_sub(y),
        ),
        _ => (y, width.saturating_sub(1).saturating_sub(x)),
    }
}
