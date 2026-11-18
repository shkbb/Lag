//! Crop / reframe box math: display <-> source transforms (rotation, flips) and the drag rules of the
//! overlay handles (snapping, clamping, minimum size).

use super::model::*;

pub fn forward_transform_rect(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    flip_h: bool,
    flip_v: bool,
    rotation: i32,
) -> (f64, f64, f64, f64) {
    let (mut l, mut t, mut r, mut b) = (x, y, x + w, y + h);
    if flip_h {
        (l, r) = (1.0 - r, 1.0 - l);
    }
    if flip_v {
        (t, b) = (1.0 - b, 1.0 - t);
    }
    let (dl, dt, dr, db) = match rotation {
        90 => (1.0 - b, l, 1.0 - t, r),
        180 => (1.0 - r, 1.0 - b, 1.0 - l, 1.0 - t),
        270 => (t, 1.0 - r, b, 1.0 - l),
        _ => (l, t, r, b),
    };
    (dl, dt, dr - dl, db - dt)
}

pub fn inverse_transform_rect(
    dx: f64,
    dy: f64,
    dw: f64,
    dh: f64,
    flip_h: bool,
    flip_v: bool,
    rotation: i32,
) -> (f64, f64, f64, f64) {
    let (dl, dt, dr, db) = (dx, dy, dx + dw, dy + dh);
    let (mut l, mut t, mut r, mut b) = match rotation {
        90 => (dt, 1.0 - dr, db, 1.0 - dl),
        180 => (1.0 - dr, 1.0 - db, 1.0 - dl, 1.0 - dt),
        270 => (1.0 - db, dl, 1.0 - dt, dr),
        _ => (dl, dt, dr, db),
    };
    if flip_v {
        (t, b) = (1.0 - b, 1.0 - t);
    }
    if flip_h {
        (l, r) = (1.0 - r, 1.0 - l);
    }
    (l, t, r - l, b - t)
}

pub fn apply_overlay_drag(
    model: &mut EditModel,
    start_disp: (f64, f64, f64, f64),
    drag_mode: i32,
    dx_px: f64,
    dy_px: f64,
    refr: (f64, f64, f64, f64),
    stage_size: (f64, f64),
) {
    let (rx, ry, rw, rh) = refr;
    let (sw, sh) = stage_size;
    if rw <= 0.0 || rh <= 0.0 {
        return;
    }

    let ndx = dx_px / rw;
    let ndy = dy_px / rh;
    let (mut l, mut t, mut r, mut b) =
        (start_disp.0, start_disp.1, start_disp.0 + start_disp.2, start_disp.1 + start_disp.3);

    let is_left = matches!(drag_mode, 5 | 7 | 9);
    let is_right = matches!(drag_mode, 4 | 6 | 8);
    let is_top = matches!(drag_mode, 2 | 6 | 7);
    let is_bottom = matches!(drag_mode, 3 | 8 | 9);

    if drag_mode == 1 {
        l += ndx;
        t += ndy;
        r += ndx;
        b += ndy;
    } else {
        if is_left {
            l += ndx;
        }
        if is_right {
            r += ndx;
        }
        if is_top {
            t += ndy;
        }
        if is_bottom {
            b += ndy;
        }
    }

    if model.crop_tool_active {
        enforce_min(&mut l, &mut t, &mut r, &mut b, is_left, is_top);
        let (nx, ny, nw, nh) = inverse_transform_rect(l, t, r - l, b - t, model.flip_h, model.flip_v, model.rotation);
        model.set_crop(nx, ny, nw, nh);
        return;
    }

    let snap_x = 9.0 / rw;
    let snap_y = 9.0 / rh;
    let min_x = -rx / rw;
    let max_x = (sw - rx) / rw;
    let min_y = -ry / rh;
    let max_y = (sh - ry) / rh;

    if drag_mode == 1 {
        if l.abs() < snap_x {
            let o = -l;
            l += o;
            r += o;
        } else if (r - 1.0).abs() < snap_x {
            let o = 1.0 - r;
            l += o;
            r += o;
        }
        if t.abs() < snap_y {
            let o = -t;
            t += o;
            b += o;
        } else if (b - 1.0).abs() < snap_y {
            let o = 1.0 - b;
            t += o;
            b += o;
        }
        if l < min_x {
            r += min_x - l;
            l = min_x;
        }
        if r > max_x {
            l -= r - max_x;
            r = max_x;
        }
        if t < min_y {
            b += min_y - t;
            t = min_y;
        }
        if b > max_y {
            t -= b - max_y;
            b = max_y;
        }
    } else {
        if is_left {
            if l.abs() < snap_x {
                l = 0.0;
            }
            l = l.max(min_x);
        }
        if is_right {
            if (r - 1.0).abs() < snap_x {
                r = 1.0;
            }
            r = r.min(max_x);
        }
        if is_top {
            if t.abs() < snap_y {
                t = 0.0;
            }
            t = t.max(min_y);
        }
        if is_bottom {
            if (b - 1.0).abs() < snap_y {
                b = 1.0;
            }
            b = b.min(max_y);
        }
        enforce_min(&mut l, &mut t, &mut r, &mut b, is_left, is_top);
    }

    let (nx, ny, nw, nh) = inverse_transform_rect(l, t, r - l, b - t, model.flip_h, model.flip_v, model.rotation);
    model.set_video_rect(nx, ny, nw, nh);
}

pub(super) fn enforce_min(l: &mut f64, t: &mut f64, r: &mut f64, b: &mut f64, is_left: bool, is_top: bool) {
    if *r - *l < MIN_NORM_SIZE {
        if is_left {
            *l = *r - MIN_NORM_SIZE;
        } else {
            *r = *l + MIN_NORM_SIZE;
        }
    }
    if *b - *t < MIN_NORM_SIZE {
        if is_top {
            *t = *b - MIN_NORM_SIZE;
        } else {
            *b = *t + MIN_NORM_SIZE;
        }
    }
}
