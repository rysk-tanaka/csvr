use gpui::{Bounds, PathBuilder, Window, fill, point, px, rgb, size};

use crate::data::{ChartData, ChartType};

pub(crate) const CHART_BLUE: u32 = 0x89b4fa;
pub(crate) const CHART_GREEN: u32 = 0xa6e3a1;
pub(crate) const CHART_PEACH: u32 = 0xfab387;
pub(crate) const CHART_MAUVE: u32 = 0xcba6f7;
pub(crate) const CHART_CANVAS_HEIGHT: f32 = 280.0;

pub(crate) fn draw_chart(
    window: &mut Window,
    bounds: Bounds<gpui::Pixels>,
    data: &ChartData,
    chart_type: ChartType,
) {
    let padding = 16.0;
    let x0 = bounds.origin.x.0 + padding;
    let y0 = bounds.origin.y.0 + padding;
    let w = bounds.size.width.0 - padding * 2.0;
    let h = bounds.size.height.0 - padding * 2.0;
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    match (chart_type, data) {
        (ChartType::Bar, ChartData::Points(vals)) | (ChartType::Line, ChartData::Points(vals))
            if vals.is_empty() => {}

        (ChartType::Bar, ChartData::Points(vals)) => {
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let base_min = min.min(0.0);
            let effective_range = max - base_min;
            let effective_range = if effective_range.abs() < f64::EPSILON { 1.0 } else { effective_range };

            let n = vals.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &val) in vals.iter().enumerate() {
                let normalized = (val - base_min) / effective_range;
                let bar_h = (normalized as f32 * h).max(1.0);
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_BLUE)));
            }
        }

        (ChartType::Line, ChartData::Points(vals)) => {
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let range = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };

            let n = vals.len();
            let points: Vec<(f32, f32)> = vals
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let px_x = if n > 1 { x0 + i as f32 * w / (n - 1) as f32 } else { x0 + w / 2.0 };
                    let normalized = (v - min) / range;
                    let px_y = y0 + h - normalized as f32 * h;
                    (px_x, px_y)
                })
                .collect();

            // Draw line
            if points.len() >= 2 {
                let mut path = PathBuilder::stroke(px(2.0));
                path.move_to(point(px(points[0].0), px(points[0].1)));
                for &(px_x, px_y) in &points[1..] {
                    path.line_to(point(px(px_x), px(px_y)));
                }
                match path.build() {
                    Ok(path) => window.paint_path(path, rgb(CHART_GREEN)),
                    Err(e) => {
                        eprintln!("Warning: failed to build line chart path: {e:?}");
                    }
                }
            }

            // Draw dots
            let dot_r = 2.5_f32;
            for &(px_x, px_y) in &points {
                let dot = Bounds::new(
                    point(px(px_x - dot_r), px(px_y - dot_r)),
                    size(px(dot_r * 2.0), px(dot_r * 2.0)),
                );
                window.paint_quad(fill(dot, rgb(CHART_GREEN)).corner_radii(px(dot_r)));
            }
        }

        (ChartType::Histogram, ChartData::Bins(bins)) => {
            let max_count = bins.iter().copied().max().unwrap_or(1).max(1);
            let n = bins.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &count) in bins.iter().enumerate() {
                let bar_h = (count as f32 / max_count as f32 * h).max(if count > 0 { 1.0 } else { 0.0 });
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_MAUVE)));
            }
        }

        (ChartType::Scatter, ChartData::Pairs(pairs)) if pairs.is_empty() => {}

        (ChartType::Scatter, ChartData::Pairs(pairs)) => {
            let x_min = pairs.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let x_max = pairs.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let y_min = pairs.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let y_max = pairs.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            let x_range = if (x_max - x_min).abs() < f64::EPSILON { 1.0 } else { x_max - x_min };
            let y_range = if (y_max - y_min).abs() < f64::EPSILON { 1.0 } else { y_max - y_min };

            let dot_r = 3.0_f32;
            for &(xv, yv) in pairs {
                let px_x = x0 + ((xv - x_min) / x_range) as f32 * w;
                let px_y = y0 + h - ((yv - y_min) / y_range) as f32 * h;
                let dot = Bounds::new(
                    point(px(px_x - dot_r), px(px_y - dot_r)),
                    size(px(dot_r * 2.0), px(dot_r * 2.0)),
                );
                window.paint_quad(fill(dot, rgb(CHART_PEACH)).corner_radii(px(dot_r)));
            }
        }

        (ChartType::Bar, ChartData::Bins(_) | ChartData::Pairs(_))
        | (ChartType::Line, ChartData::Bins(_) | ChartData::Pairs(_))
        | (ChartType::Histogram, ChartData::Points(_) | ChartData::Pairs(_))
        | (ChartType::Scatter, ChartData::Points(_) | ChartData::Bins(_)) => {
            debug_assert!(false, "Mismatched ChartType and ChartData");
            eprintln!("Bug: mismatched ChartType and ChartData variant");
        }
    }
}
