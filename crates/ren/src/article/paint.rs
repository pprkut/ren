// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! A painter between Blitz and the renderer that draws the text selection
//! in ren's colour.
//!
//! Blitz paints selected text on a fixed light blue (`SELECTION_COLOR` in
//! `blitz-paint`, (180, 213, 255)) and doesn't support `::selection`. In
//! dark mode, the light text is hard to read on it. So fills in exactly
//! that colour are drawn in the colour given instead; an author's
//! background of exactly that colour would change too.

use std::sync::Arc;

use anyrender::{Filter, Glyph, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, Rect, Shape, Stroke};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};

/// Blitz's selection colour.
const BLITZ_SELECTION: [u8; 4] = [180, 213, 255, 255];

pub struct SelectionPainter<'p, P> {
    pub inner: &'p mut P,
    pub selection: Color,
}

impl<P: PaintScene> RenderContext for SelectionPainter<'_, P> {}

impl<P: PaintScene> PaintScene for SelectionPainter<'_, P> {
    fn reset(&mut self) {
        self.inner.reset();
    }

    fn push_layer(
        &mut self,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop_filter: Option<Arc<Filter>>,
    ) {
        self.inner
            .push_layer(blend, alpha, transform, clip, filter, backdrop_filter);
    }

    fn push_clip_layer(&mut self, transform: Affine, clip: &impl Shape) {
        self.inner.push_clip_layer(transform, clip);
    }

    fn pop_layer(&mut self) {
        self.inner.pop_layer();
    }

    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.inner
            .stroke(style, transform, brush, brush_transform, shape);
    }

    fn fill<'a>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let brush = match brush.into() {
            Paint::Solid(color) if color.to_rgba8().to_u8_array() == BLITZ_SELECTION => {
                Paint::Solid(self.selection)
            }
            brush => brush,
        };
        self.inner
            .fill(style, transform, brush, brush_transform, shape);
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        embolden: kurbo::Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        self.inner.draw_glyphs(
            font,
            font_size,
            hint,
            normalized_coords,
            embolden,
            style,
            brush,
            brush_alpha,
            transform,
            glyph_transform,
            glyphs,
        );
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.inner
            .draw_box_shadow(transform, rect, brush, radius, std_dev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::Scene;
    use anyrender::recording::RenderCommand;

    fn filled(color: Color) -> Option<Color> {
        let mut scene = Scene::new();
        let mut painter = SelectionPainter {
            inner: &mut scene,
            selection: Color::from_rgb8(1, 2, 3),
        };
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color,
            None,
            &Rect::new(0.0, 0.0, 1.0, 1.0),
        );
        match scene.commands.first()? {
            RenderCommand::Fill(cmd) => match cmd.brush {
                Paint::Solid(color) => Some(color),
                _ => None,
            },
            _ => None,
        }
    }

    #[test]
    fn only_the_selection_colour_is_replaced() {
        assert_eq!(
            filled(Color::from_rgb8(180, 213, 255)),
            Some(Color::from_rgb8(1, 2, 3))
        );
        assert_eq!(
            filled(Color::from_rgb8(180, 213, 254)),
            Some(Color::from_rgb8(180, 213, 254))
        );
        assert_eq!(
            filled(Color::from_rgba8(180, 213, 255, 128)),
            Some(Color::from_rgba8(180, 213, 255, 128))
        );
    }
}
