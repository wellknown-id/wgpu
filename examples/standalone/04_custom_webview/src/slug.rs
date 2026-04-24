use std::collections::HashMap;

const TEXTURE_WIDTH: usize = 4096;
const NUM_BANDS: usize = 8;

#[derive(Clone, Copy, Debug)]
struct QuadCurve {
    p1: [f32; 2],
    p2: [f32; 2],
    p3: [f32; 2],
}

impl QuadCurve {
    fn max_x(&self) -> f32 {
        self.p1[0].max(self.p2[0]).max(self.p3[0])
    }

    fn max_y(&self) -> f32 {
        self.p1[1].max(self.p2[1]).max(self.p3[1])
    }

    fn min_x(&self) -> f32 {
        self.p1[0].min(self.p2[0]).min(self.p3[0])
    }

    fn min_y(&self) -> f32 {
        self.p1[1].min(self.p2[1]).min(self.p3[1])
    }
}

#[derive(Clone, Debug)]
pub struct GlyphData {
    pub bbox: [f32; 4],
    pub glyph_loc: [u32; 2],
    pub band_max: [u32; 2],
    pub band_transform: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct FontAtlas {
    pub glyphs: HashMap<u16, GlyphData>,
    pub curve_texels: Vec<[f32; 4]>,
    pub curve_height: u32,
    pub band_texels: Vec<[u32; 2]>,
    pub band_height: u32,
}

struct OutlineCollector {
    curves: Vec<QuadCurve>,
    first: [f32; 2],
    current: [f32; 2],
}

impl OutlineCollector {
    fn new() -> Self {
        Self {
            curves: Vec::new(),
            first: [0.0; 2],
            current: [0.0; 2],
        }
    }
}

impl ttf_parser::OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.first = [x, y];
        self.current = [x, y];
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p1 = self.current;
        let p3 = [x, y];
        let p2 = [(p1[0] + p3[0]) * 0.5, (p1[1] + p3[1]) * 0.5];
        self.curves.push(QuadCurve { p1, p2, p3 });
        self.current = p3;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p1 = self.current;
        let p2 = [x1, y1];
        let p3 = [x, y];
        self.curves.push(QuadCurve { p1, p2, p3 });
        self.current = p3;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.current;
        let c1 = [x1, y1];
        let c2 = [x2, y2];
        let p3 = [x, y];

        let mid = [
            0.125 * (p0[0] + 3.0 * c1[0] + 3.0 * c2[0] + p3[0]),
            0.125 * (p0[1] + 3.0 * c1[1] + 3.0 * c2[1] + p3[1]),
        ];

        self.curves.push(QuadCurve {
            p1: p0,
            p2: [0.5 * (p0[0] + c1[0]), 0.5 * (p0[1] + c1[1])],
            p3: mid,
        });
        self.curves.push(QuadCurve {
            p1: mid,
            p2: [0.5 * (c2[0] + p3[0]), 0.5 * (c2[1] + p3[1])],
            p3,
        });

        self.current = p3;
    }

    fn close(&mut self) {
        if self.current != self.first {
            self.line_to(self.first[0], self.first[1]);
        }
    }
}

pub fn build_font_atlas(font_data: &[u8], face_index: u32) -> Result<FontAtlas, String> {
    let face = ttf_parser::Face::parse(font_data, face_index)
        .map_err(|e| format!("failed to parse font face {face_index}: {e}"))?;
    let upem = face.units_per_em() as f32;

    let glyph_ids: Vec<u16> = (0..face.number_of_glyphs()).collect();
    let mut glyph_curves: HashMap<u16, Vec<QuadCurve>> = HashMap::with_capacity(glyph_ids.len());

    for gid in &glyph_ids {
        let mut collector = OutlineCollector::new();
        if face
            .outline_glyph(ttf_parser::GlyphId(*gid), &mut collector)
            .is_some()
        {
            for curve in &mut collector.curves {
                curve.p1[0] /= upem;
                curve.p1[1] /= upem;
                curve.p2[0] /= upem;
                curve.p2[1] /= upem;
                curve.p3[0] /= upem;
                curve.p3[1] /= upem;
            }
            glyph_curves.insert(*gid, collector.curves);
        } else {
            glyph_curves.insert(*gid, Vec::new());
        }
    }

    let mut curve_row = 0usize;
    let mut curve_col = 0usize;
    let mut curve_locations: HashMap<u16, Vec<(usize, usize, usize)>> =
        HashMap::with_capacity(glyph_ids.len());

    for gid in &glyph_ids {
        let curves = &glyph_curves[gid];
        let mut locations = Vec::with_capacity(curves.len());
        for (index, _) in curves.iter().enumerate() {
            if curve_col + 2 > TEXTURE_WIDTH {
                curve_col = 0;
                curve_row += 1;
            }
            locations.push((index, curve_col, curve_row));
            curve_col += 2;
        }
        curve_locations.insert(*gid, locations);
    }

    let curve_height = (curve_row + 1).max(1);
    let mut curve_texels = vec![[0.0; 4]; curve_height * TEXTURE_WIDTH];
    for gid in &glyph_ids {
        let curves = &glyph_curves[gid];
        let locations = &curve_locations[gid];
        for (index, curve) in curves.iter().enumerate() {
            let (_, col, row) = locations[index];
            curve_texels[row * TEXTURE_WIDTH + col] =
                [curve.p1[0], curve.p1[1], curve.p2[0], curve.p2[1]];
            curve_texels[row * TEXTURE_WIDTH + col + 1] = [curve.p3[0], curve.p3[1], 0.0, 0.0];
        }
    }

    let mut band_texels: Vec<[u32; 2]> = Vec::new();
    let mut band_row = 0usize;
    let mut band_col = 0usize;
    let mut glyphs = HashMap::with_capacity(glyph_ids.len());

    for gid in glyph_ids {
        let curves = &glyph_curves[&gid];
        if curves.is_empty() {
            glyphs.insert(
                gid,
                GlyphData {
                    bbox: [0.0; 4],
                    glyph_loc: [0, 0],
                    band_max: [0, 0],
                    band_transform: [0.0; 4],
                },
            );
            continue;
        }

        let mut x_min = f32::MAX;
        let mut y_min = f32::MAX;
        let mut x_max = f32::MIN;
        let mut y_max = f32::MIN;
        for curve in curves {
            x_min = x_min.min(curve.min_x());
            y_min = y_min.min(curve.min_y());
            x_max = x_max.max(curve.max_x());
            y_max = y_max.max(curve.max_y());
        }

        let width = x_max - x_min;
        let height = y_max - y_min;
        let band_scale_x = if width > 0.0 {
            (NUM_BANDS as f32 - 0.001) / width
        } else {
            0.0
        };
        let band_scale_y = if height > 0.0 {
            (NUM_BANDS as f32 - 0.001) / height
        } else {
            0.0
        };
        let band_offset_x = -x_min * band_scale_x;
        let band_offset_y = -y_min * band_scale_y;

        let mut h_bands: Vec<Vec<usize>> = vec![Vec::new(); NUM_BANDS];
        for (curve_index, curve) in curves.iter().enumerate() {
            let start = ((curve.min_y() - y_min) * band_scale_y).floor().max(0.0) as usize;
            let end = ((curve.max_y() - y_min) * band_scale_y).floor().max(0.0) as usize;
            for band in h_bands
                .iter_mut()
                .take(end.min(NUM_BANDS - 1) + 1)
                .skip(start)
            {
                band.push(curve_index);
            }
        }
        for band in &mut h_bands {
            band.sort_by(|&left, &right| {
                curves[right]
                    .max_x()
                    .partial_cmp(&curves[left].max_x())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let mut v_bands: Vec<Vec<usize>> = vec![Vec::new(); NUM_BANDS];
        for (curve_index, curve) in curves.iter().enumerate() {
            let start = ((curve.min_x() - x_min) * band_scale_x).floor().max(0.0) as usize;
            let end = ((curve.max_x() - x_min) * band_scale_x).floor().max(0.0) as usize;
            for band in v_bands
                .iter_mut()
                .take(end.min(NUM_BANDS - 1) + 1)
                .skip(start)
            {
                band.push(curve_index);
            }
        }
        for band in &mut v_bands {
            band.sort_by(|&left, &right| {
                curves[right]
                    .max_y()
                    .partial_cmp(&curves[left].max_y())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let header_size = NUM_BANDS * 2;
        let total_refs = h_bands.iter().map(Vec::len).sum::<usize>()
            + v_bands.iter().map(Vec::len).sum::<usize>();
        let total_glyph_texels = header_size + total_refs;
        if band_col + total_glyph_texels > TEXTURE_WIDTH {
            band_col = 0;
            band_row += 1;
        }

        let glyph_start_col = band_col;
        let glyph_start_row = band_row;
        let mut glyph_band_data: Vec<[u32; 2]> = Vec::with_capacity(total_glyph_texels);
        let mut curve_list_offset = header_size;

        for band in &h_bands {
            glyph_band_data.push([band.len() as u32, curve_list_offset as u32]);
            curve_list_offset += band.len();
        }
        for band in &v_bands {
            glyph_band_data.push([band.len() as u32, curve_list_offset as u32]);
            curve_list_offset += band.len();
        }

        let curve_locs = &curve_locations[&gid];
        for band in &h_bands {
            for &curve_index in band {
                let (_, col, row) = curve_locs[curve_index];
                glyph_band_data.push([col as u32, row as u32]);
            }
        }
        for band in &v_bands {
            for &curve_index in band {
                let (_, col, row) = curve_locs[curve_index];
                glyph_band_data.push([col as u32, row as u32]);
            }
        }

        let needed_rows =
            glyph_start_row + 1 + (glyph_start_col + total_glyph_texels) / TEXTURE_WIDTH;
        while band_texels.len() < needed_rows * TEXTURE_WIDTH {
            band_texels.resize(band_texels.len() + TEXTURE_WIDTH, [0; 2]);
        }

        let mut write_col = glyph_start_col;
        let mut write_row = glyph_start_row;
        for texel in glyph_band_data {
            if write_col >= TEXTURE_WIDTH {
                write_col = 0;
                write_row += 1;
                while band_texels.len() < (write_row + 1) * TEXTURE_WIDTH {
                    band_texels.resize(band_texels.len() + TEXTURE_WIDTH, [0; 2]);
                }
            }
            band_texels[write_row * TEXTURE_WIDTH + write_col] = texel;
            write_col += 1;
        }
        band_col = write_col;
        band_row = write_row;

        glyphs.insert(
            gid,
            GlyphData {
                bbox: [x_min, y_min, x_max, y_max],
                glyph_loc: [glyph_start_col as u32, glyph_start_row as u32],
                band_max: [(NUM_BANDS - 1) as u32, (NUM_BANDS - 1) as u32],
                band_transform: [band_scale_x, band_scale_y, band_offset_x, band_offset_y],
            },
        );
    }

    let band_height = (band_texels.len() / TEXTURE_WIDTH).max(1);
    band_texels.resize(band_height * TEXTURE_WIDTH, [0; 2]);

    Ok(FontAtlas {
        glyphs,
        curve_texels,
        curve_height: curve_height as u32,
        band_texels,
        band_height: band_height as u32,
    })
}
