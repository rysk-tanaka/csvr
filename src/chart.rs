use gpui::{
    App, Bounds, ContentMask, PathBuilder, Pixels, ShapedLine, SharedString, TextAlign, TextRun,
    Window, fill, point, px, rems, rgb, size,
};

use crate::data::{ChartAxes, ChartData, ChartType};

pub(crate) const CHART_BLUE: u32 = 0x89b4fa;
pub(crate) const CHART_GREEN: u32 = 0xa6e3a1;
pub(crate) const CHART_PEACH: u32 = 0xfab387;
pub(crate) const CHART_MAUVE: u32 = 0xcba6f7;
pub(crate) const CHART_CANVAS_HEIGHT: f32 = 280.0;
const CHART_LABEL: u32 = 0xa6adc8; // Same as TEXT_SUBTEXT in app.rs
const CHART_GRID: u32 = 0x45475a; // Same as BORDER_COLOR in app.rs

const PADDING: f32 = 16.0;
const LABEL_GAP: f32 = 6.0;
const LEGEND_TOP: f32 = 6.0;
const LEGEND_SWATCH: f32 = 8.0;
const LINE_HEIGHT_RATIO: f32 = 1.4;

fn shape_label(window: &Window, text: &str, font_size: Pixels) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color: rgb(CHART_LABEL).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_string()),
        font_size,
        &[run],
        None,
    )
}

fn paint_label(
    line: &ShapedLine,
    x: f32,
    y: f32,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = point(px(x), px(y));
    if let Err(e) = line.paint(origin, line_height, TextAlign::Left, None, window, cx) {
        eprintln!("Warning: failed to paint chart label: {e:?}");
    }
}

fn series_color(chart_type: ChartType) -> u32 {
    match chart_type {
        ChartType::Bar => CHART_BLUE,
        ChartType::Line => CHART_GREEN,
        ChartType::Scatter => CHART_PEACH,
        ChartType::Histogram => CHART_MAUVE,
    }
}

pub(crate) fn draw_chart(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    data: &ChartData,
    chart_type: ChartType,
    axes: &ChartAxes,
    legend: &str,
) {
    let font_size = rems(0.75).to_pixels(window.rem_size());
    let line_height = font_size.as_f32() * LINE_HEIGHT_RATIO;
    let line_height_px = px(line_height);

    let y_labels: Vec<(f32, ShapedLine)> = axes
        .y
        .ticks
        .iter()
        .map(|(pos, label)| (*pos, shape_label(window, label, font_size)))
        .collect();
    let x_labels: Vec<(f32, ShapedLine)> = axes
        .x
        .iter()
        .flat_map(|axis| &axis.ticks)
        .map(|(pos, label)| (*pos, shape_label(window, label, font_size)))
        .collect();
    let y_label_width = y_labels
        .iter()
        .map(|(_, line)| line.width().as_f32())
        .fold(0.0, f32::max);

    let left = bounds.origin.x.as_f32();
    let top = bounds.origin.y.as_f32();
    let x0 = left + PADDING + y_label_width + LABEL_GAP;
    let y0 = top + LEGEND_TOP + line_height + line_height / 2.0;
    let bottom = if x_labels.is_empty() {
        PADDING
    } else {
        line_height + LABEL_GAP * 2.0
    };
    let w = left + bounds.size.width.as_f32() - PADDING - x0;
    let h = top + bounds.size.height.as_f32() - bottom - y0;
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (pos, _) in &y_labels {
            let y = y0 + h - pos * h;
            let line = Bounds::new(point(px(x0), px(y)), size(px(w), px(1.0)));
            window.paint_quad(fill(line, rgb(CHART_GRID)));
        }

        let plot = Bounds::new(point(px(x0), px(y0)), size(px(w), px(h)));
        draw_series(window, plot, data, chart_type, axes);

        let swatch_x = left + PADDING;
        let legend_y = top + LEGEND_TOP;
        let swatch = Bounds::new(
            point(
                px(swatch_x),
                px(legend_y + (line_height - LEGEND_SWATCH) / 2.0),
            ),
            size(px(LEGEND_SWATCH), px(LEGEND_SWATCH)),
        );
        window.paint_quad(fill(swatch, rgb(series_color(chart_type))));
        let legend_line = shape_label(window, legend, font_size);
        paint_label(
            &legend_line,
            swatch_x + LEGEND_SWATCH + LABEL_GAP,
            legend_y,
            line_height_px,
            window,
            cx,
        );

        for (pos, line) in &y_labels {
            let x = x0 - LABEL_GAP - line.width().as_f32();
            let y = y0 + h - pos * h - line_height / 2.0;
            paint_label(line, x, y, line_height_px, window, cx);
        }

        // The position spreads the labels so the first and last stay within the plot.
        let placed: Vec<(f32, &ShapedLine)> = x_labels
            .iter()
            .map(|(pos, line)| (x0 + pos * (w - line.width().as_f32()), line))
            .collect();
        let last_start = placed.last().map_or(x0 + w, |(x, _)| *x);
        let mut previous_end = f32::NEG_INFINITY;
        for (i, (x, line)) in placed.iter().enumerate() {
            let end = x + line.width().as_f32();
            let is_middle = i > 0 && i + 1 < placed.len();
            let overlaps_previous = *x < previous_end + LABEL_GAP;
            let overlaps_last = is_middle && end + LABEL_GAP > last_start;
            if overlaps_previous || overlaps_last {
                continue;
            }
            previous_end = end;
            paint_label(line, *x, y0 + h + LABEL_GAP, line_height_px, window, cx);
        }
    });
}

fn draw_series(
    window: &mut Window,
    plot: Bounds<Pixels>,
    data: &ChartData,
    chart_type: ChartType,
    axes: &ChartAxes,
) {
    let x0 = plot.origin.x.as_f32();
    let y0 = plot.origin.y.as_f32();
    let w = plot.size.width.as_f32();
    let h = plot.size.height.as_f32();
    let y_lo = axes.y.lo;
    let y_range = axes.y.hi - axes.y.lo;

    match (chart_type, data) {
        (ChartType::Bar, ChartData::Points(vals)) | (ChartType::Line, ChartData::Points(vals))
            if vals.is_empty() => {}

        (ChartType::Bar, ChartData::Points(vals)) => {
            let n = vals.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &val) in vals.iter().enumerate() {
                let normalized = (val - y_lo) / y_range;
                let bar_h = (normalized as f32 * h).max(1.0);
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_BLUE)));
            }
        }

        (ChartType::Line, ChartData::Points(vals)) => {
            let n = vals.len();
            let points: Vec<(f32, f32)> = vals
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let px_x = if n > 1 {
                        x0 + i as f32 * w / (n - 1) as f32
                    } else {
                        x0 + w / 2.0
                    };
                    let normalized = (v - y_lo) / y_range;
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
            let max_count = axes.y.hi as f32;
            let n = bins.len();
            let gap = 1.0_f32;
            let bar_w = ((w - gap * (n as f32 - 1.0).max(0.0)) / n as f32).max(1.0);

            for (i, &count) in bins.iter().enumerate() {
                let bar_h = (count as f32 / max_count * h).max(if count > 0 { 1.0 } else { 0.0 });
                let bx = x0 + i as f32 * (bar_w + gap);
                let by = y0 + h - bar_h;
                let rect = Bounds::new(point(px(bx), px(by)), size(px(bar_w), px(bar_h)));
                window.paint_quad(fill(rect, rgb(CHART_MAUVE)));
            }
        }

        (ChartType::Scatter, ChartData::Pairs(pairs)) if pairs.is_empty() => {}

        (ChartType::Scatter, ChartData::Pairs(pairs)) => {
            let (x_lo, x_range) = axes
                .x
                .as_ref()
                .map_or((0.0, 1.0), |axis| (axis.lo, axis.hi - axis.lo));

            let dot_r = 3.0_f32;
            for &(xv, yv) in pairs {
                let px_x = x0 + ((xv - x_lo) / x_range) as f32 * w;
                let px_y = y0 + h - ((yv - y_lo) / y_range) as f32 * h;
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
