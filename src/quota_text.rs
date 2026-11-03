//! Fixed-column typography for the compact quota display.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;

use crate::native_interop::{self, Color};

const DIGIT_CELL: i32 = 8;
const PERCENT_CELL: i32 = 12;
pub const COLUMN_GAP: i32 = 6;
/// Cell widths are tuned for 12 px text and grow with larger text.
const BASE_FONT_PX: i32 = 12;

fn cell(width: i32, font_px: i32) -> i32 {
    (width * font_px + BASE_FONT_PX - 1) / BASE_FONT_PX
}

fn percent_width(font_px: i32) -> i32 {
    cell(DIGIT_CELL, font_px) * 3 + cell(PERCENT_CELL, font_px)
}

/// Width of the percentage column without reset/countdown text.
pub fn percent_column_width(font_px: i32) -> i32 {
    percent_width(font_px)
}

/// Logical offset of the text after the percentage column.
pub fn reset_offset(font_px: i32) -> i32 {
    percent_width(font_px) + COLUMN_GAP
}

/// Where the reset string starts once an optional kind label has been placed
/// immediately after the percentage column. `percent_right` and `gap` are the
/// same pixel space as `kind_width`.
pub fn reset_origin(percent_right: i32, kind_width: i32, gap: i32) -> i32 {
    if kind_width <= 0 {
        percent_right
    } else {
        percent_right + kind_width + gap
    }
}

/// `height` is in physical pixels. `face` is a Windows font family name.
pub unsafe fn create_reset_font(height: i32, face: &str) -> HFONT {
    let face = native_interop::wide_str(face);
    CreateFontW(
        -height,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_TT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        PCWSTR(face.as_ptr()),
    )
}

fn parts(text: &str) -> (&str, &str) {
    if let Some(end) = text.find('%') {
        if text[..end].chars().all(|c| c.is_ascii_digit()) && end > 0 {
            return (
                &text[..=end],
                text[end + 1..].trim_start_matches([' ', '·']),
            );
        }
    }
    (text, "")
}

fn percentage_cells(text: &str) -> Option<String> {
    let digits = text.strip_suffix('%')?;
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{digits:>3}%"))
}

fn text(hdc: HDC, value: &str, mut rect: RECT, flags: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut rect,
            flags | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
}

/// Each digit occupies a fixed cell; kind and reset text start at the same x
/// for 0%, 9%, 99% and 100%. Status labels remain in the percentage column.
/// `kind` is drawn in the primary color between the percentage and the reset.
/// `x`, `y`, `height` and `width` are physical pixels; `font_px` is logical.
pub fn draw(
    hdc: HDC,
    x: i32,
    y: i32,
    height: i32,
    width: i32,
    value: &str,
    kind: Option<&str>,
    primary: &Color,
    secondary: &Color,
    font_px: i32,
    font_face: &str,
    scale: impl Fn(i32) -> i32,
) {
    let (percentage, reset) = parts(value);
    let rect = RECT {
        left: x,
        top: y,
        right: x + scale(percent_width(font_px)),
        bottom: y + height,
    };
    unsafe {
        let _ = SetTextColor(hdc, COLORREF(primary.to_colorref()));
    }
    if let Some(cells) = percentage_cells(percentage) {
        for (index, glyph) in cells.chars().enumerate() {
            if glyph == ' ' {
                continue;
            }
            let left = x + scale(index as i32 * cell(DIGIT_CELL, font_px));
            let cell_width = cell(
                if glyph == '%' {
                    PERCENT_CELL
                } else {
                    DIGIT_CELL
                },
                font_px,
            );
            text(
                hdc,
                &glyph.to_string(),
                RECT {
                    left,
                    right: left + scale(cell_width),
                    ..rect
                },
                DT_CENTER,
            );
        }
    } else {
        text(hdc, percentage, rect, DT_RIGHT);
    }

    let kind = kind.unwrap_or("").trim();
    if kind.is_empty() && reset.is_empty() {
        return;
    }

    let percent_right = x + scale(reset_offset(font_px));
    let gap = scale(COLUMN_GAP);
    unsafe {
        let _ = SetTextColor(hdc, COLORREF(primary.to_colorref()));
        let kind_width = if kind.is_empty() {
            0
        } else {
            let wide: Vec<u16> = kind.encode_utf16().collect();
            let mut size = SIZE::default();
            let measured = GetTextExtentPoint32W(hdc, &wide, &mut size).as_bool();
            text(
                hdc,
                kind,
                RECT {
                    left: percent_right,
                    right: x + width,
                    ..rect
                },
                DT_LEFT,
            );
            if measured { size.cx } else { 0 }
        };
        if reset.is_empty() {
            return;
        }
        let reset_x = reset_origin(percent_right, kind_width, gap);
        let font = create_reset_font(scale(font_px), font_face);
        let old_font = SelectObject(hdc, font);
        let _ = SetTextColor(hdc, COLORREF(secondary.to_colorref()));
        text(
            hdc,
            reset,
            RECT {
                left: reset_x,
                right: x + width,
                ..rect
            },
            DT_LEFT,
        );
        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larger_text_widens_cells_without_moving_standard_columns() {
        assert_eq!(reset_offset(12), 8 * 3 + 12 + 6);
        assert_eq!(cell(DIGIT_CELL, 13), 9);
        assert_eq!(reset_offset(13), 9 * 3 + 13 + 6);
    }

    #[test]
    fn percentages_keep_the_same_digit_and_percent_slots() {
        assert_eq!(percentage_cells("2%"), Some("  2%".into()));
        assert_eq!(percentage_cells("87%"), Some(" 87%".into()));
        assert_eq!(percentage_cells("100%"), Some("100%".into()));
        assert_eq!(percentage_cells("--"), None);
    }

    #[test]
    fn kind_label_shifts_the_reset_and_an_absent_kind_does_not() {
        assert_eq!(reset_origin(42, 0, 6), 42);
        assert_eq!(reset_origin(42, 28, 6), 76);
    }

    #[test]
    fn separates_percentage_from_reset_without_changing_status_labels() {
        assert_eq!(parts("2%  01:59重置"), ("2%", "01:59重置"));
        assert_eq!(parts("87%  10/08重置"), ("87%", "10/08重置"));
        assert_eq!(parts("18% · 2h 34m"), ("18%", "2h 34m"));
        assert_eq!(parts("--"), ("--", ""));
        assert_eq!(parts("网络"), ("网络", ""));
    }
}
