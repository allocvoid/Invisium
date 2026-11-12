//! Values in between: interpolating parsed CSS values, and reading a computed style back as
//! values so that transitions and keyframes without explicit ends have something to start from.

use crate::css::properties::{Kw, PropId, Value};
use crate::css::values::{Calc, Color, CssColor, Len, ShadowSpec, Size, TransformFn};
use crate::style::{self, ComputedStyle, TransformOp};
use std::rc::Rc;

/// Properties a transition can run on (what `transition-property: all` means here).
pub const TRANSITIONABLE: &[PropId] = &[
    PropId::Opacity,
    PropId::Transform,
    PropId::Translate,
    PropId::Rotate,
    PropId::Scale,
    PropId::Color,
    PropId::BackgroundColor,
    PropId::BorderTopColor,
    PropId::BorderRightColor,
    PropId::BorderBottomColor,
    PropId::BorderLeftColor,
    PropId::OutlineColor,
    PropId::TextFillColor,
    PropId::Width,
    PropId::Height,
    PropId::MinWidth,
    PropId::MinHeight,
    PropId::MaxWidth,
    PropId::MaxHeight,
    PropId::Top,
    PropId::Right,
    PropId::Bottom,
    PropId::Left,
    PropId::MarginTop,
    PropId::MarginRight,
    PropId::MarginBottom,
    PropId::MarginLeft,
    PropId::PaddingTop,
    PropId::PaddingRight,
    PropId::PaddingBottom,
    PropId::PaddingLeft,
    PropId::BorderTopWidth,
    PropId::BorderRightWidth,
    PropId::BorderBottomWidth,
    PropId::BorderLeftWidth,
    PropId::BorderTopLeftRadius,
    PropId::BorderTopRightRadius,
    PropId::BorderBottomRightRadius,
    PropId::BorderBottomLeftRadius,
    PropId::OutlineWidth,
    PropId::OutlineOffset,
    PropId::FontSize,
    PropId::LetterSpacing,
    PropId::BoxShadow,
    PropId::Visibility,
];

/// Properties whose animated value only changes how a box is painted (never its size or place
/// in the flow), so a frame needs no layout.
pub fn paint_only(p: PropId) -> bool {
    matches!(
        p,
        PropId::Opacity
            | PropId::Transform
            | PropId::Translate
            | PropId::Rotate
            | PropId::Scale
            | PropId::BackgroundColor
            | PropId::BorderTopColor
            | PropId::BorderRightColor
            | PropId::BorderBottomColor
            | PropId::BorderLeftColor
            | PropId::OutlineColor
            | PropId::BoxShadow
            | PropId::BorderTopLeftRadius
            | PropId::BorderTopRightRadius
            | PropId::BorderBottomRightRadius
            | PropId::BorderBottomLeftRadius
    )
}

/// Properties children take over from their parent: animating one changes the whole subtree.
pub fn inherited(p: PropId) -> bool {
    style::is_inherited(p)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn calc_of(l: &Len) -> Calc {
    match l {
        Len::Calc(c) => (**c).clone(),
        other => Calc::Leaf(other.clone()),
    }
}

pub fn lerp_len(a: &Len, b: &Len, t: f32) -> Len {
    match (a, b) {
        (Len::Px(x), Len::Px(y)) => Len::Px(lerp(*x, *y, t)),
        (Len::Pct(x), Len::Pct(y)) => Len::Pct(lerp(*x, *y, t)),
        (Len::Em(x), Len::Em(y)) => Len::Em(lerp(*x, *y, t)),
        (Len::Rem(x), Len::Rem(y)) => Len::Rem(lerp(*x, *y, t)),
        (Len::Vw(x), Len::Vw(y)) => Len::Vw(lerp(*x, *y, t)),
        (Len::Vh(x), Len::Vh(y)) => Len::Vh(lerp(*x, *y, t)),
        _ => {
            // mixed units: a * (1 - t) + b * t, evaluated when the length is used
            let part = |l: &Len, k: f32| Box::new(Calc::Mul(Box::new(calc_of(l)), Box::new(Calc::Num(k))));
            Len::Calc(Rc::new(Calc::Add(part(a, 1.0 - t), part(b, t))))
        }
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    // premultiplied, so that fading to or from transparent does not pass through a darker shade
    let (aa, ba) = (a.a as f32 / 255.0, b.a as f32 / 255.0);
    let alpha = lerp(aa, ba, t);
    if alpha <= 0.0 {
        return Color { r: 0, g: 0, b: 0, a: 0 };
    }
    let ch = |x: u8, y: u8| (lerp(x as f32 * aa, y as f32 * ba, t) / alpha).round().clamp(0.0, 255.0) as u8;
    Color { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: (alpha * 255.0).round().clamp(0.0, 255.0) as u8 }
}

fn identity_like(f: &TransformFn) -> TransformFn {
    match f {
        TransformFn::Translate(..) => TransformFn::Translate(Len::Px(0.0), Len::Px(0.0)),
        TransformFn::Scale(..) => TransformFn::Scale(1.0, 1.0),
        TransformFn::Rotate(_) => TransformFn::Rotate(0.0),
        TransformFn::Skew(..) => TransformFn::Skew(0.0, 0.0),
        TransformFn::Matrix(_) => TransformFn::Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
    }
}

fn lerp_fn(a: &TransformFn, b: &TransformFn, t: f32) -> Option<TransformFn> {
    Some(match (a, b) {
        (TransformFn::Translate(ax, ay), TransformFn::Translate(bx, by)) => TransformFn::Translate(lerp_len(ax, bx, t), lerp_len(ay, by, t)),
        (TransformFn::Scale(ax, ay), TransformFn::Scale(bx, by)) => TransformFn::Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (TransformFn::Rotate(x), TransformFn::Rotate(y)) => TransformFn::Rotate(lerp(*x, *y, t)),
        (TransformFn::Skew(ax, ay), TransformFn::Skew(bx, by)) => TransformFn::Skew(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (TransformFn::Matrix(x), TransformFn::Matrix(y)) => {
            let mut m = [0.0f32; 6];
            for i in 0..6 {
                m[i] = lerp(x[i], y[i], t);
            }
            TransformFn::Matrix(m)
        }
        _ => return None,
    })
}

fn lerp_transform(a: &[TransformFn], b: &[TransformFn], t: f32) -> Option<Vec<TransformFn>> {
    if a.is_empty() {
        return b.iter().map(|f| lerp_fn(&identity_like(f), f, t)).collect();
    }
    if b.is_empty() {
        return a.iter().map(|f| lerp_fn(f, &identity_like(f), t)).collect();
    }
    if a.len() != b.len() {
        return None;
    }
    a.iter().zip(b).map(|(x, y)| lerp_fn(x, y, t)).collect()
}

fn lerp_shadows(a: &[ShadowSpec], b: &[ShadowSpec], t: f32) -> Option<Vec<ShadowSpec>> {
    let clear = |s: &ShadowSpec| ShadowSpec {
        x: Len::Px(0.0),
        y: Len::Px(0.0),
        blur: Len::Px(0.0),
        spread: Len::Px(0.0),
        color: Some(Color { a: 0, ..s.color.unwrap_or(Color { r: 0, g: 0, b: 0, a: 0 }) }),
        inset: s.inset,
    };
    let (av, bv): (Vec<ShadowSpec>, Vec<ShadowSpec>) = if a.is_empty() {
        (b.iter().map(clear).collect(), b.to_vec())
    } else if b.is_empty() {
        (a.to_vec(), a.iter().map(clear).collect())
    } else {
        (a.to_vec(), b.to_vec())
    };
    if av.len() != bv.len() {
        return None;
    }
    av.iter()
        .zip(&bv)
        .map(|(x, y)| {
            if x.inset != y.inset {
                return None;
            }
            let color = match (x.color, y.color) {
                (Some(c), Some(d)) => Some(lerp_color(c, d, t)),
                (None, None) => None,
                _ => return None,
            };
            Some(ShadowSpec {
                x: lerp_len(&x.x, &y.x, t),
                y: lerp_len(&x.y, &y.y, t),
                blur: lerp_len(&x.blur, &y.blur, t),
                spread: lerp_len(&x.spread, &y.spread, t),
                color,
                inset: x.inset,
            })
        })
        .collect()
}

/// The value `t` of the way from `a` to `b` (`t` may leave 0..=1 when a timing function
/// overshoots). `None` if the two cannot be blended: they then switch half-way.
fn blend(a: &Value, b: &Value, t: f32) -> Option<Value> {
    Some(match (a, b) {
        (Value::Num(x), Value::Num(y)) => Value::Num(lerp(*x, *y, t)),
        (Value::Len(x), Value::Len(y)) => Value::Len(lerp_len(x, y, t)),
        (Value::Color(CssColor::Rgba(x)), Value::Color(CssColor::Rgba(y))) => {
            Value::Color(CssColor::Rgba(lerp_color(*x, *y, t.clamp(0.0, 1.0))))
        }
        (Value::Translate(ax, ay), Value::Translate(bx, by)) => Value::Translate(lerp_len(ax, bx, t), lerp_len(ay, by, t)),
        (Value::Pair(ax, ay), Value::Pair(bx, by)) => Value::Pair(lerp_len(ax, bx, t), lerp_len(ay, by, t)),
        (Value::Scale(ax, ay), Value::Scale(bx, by)) => Value::Scale(lerp(*ax, *bx, t), lerp(*ay, *by, t)),
        (Value::Transform(x), Value::Transform(y)) => Value::Transform(lerp_transform(x, y, t)?.into()),
        (Value::Kw(Kw::None), Value::Transform(y)) => Value::Transform(lerp_transform(&[], y, t)?.into()),
        (Value::Transform(x), Value::Kw(Kw::None)) => Value::Transform(lerp_transform(x, &[], t)?.into()),
        (Value::Shadows(x), Value::Shadows(y)) => Value::Shadows(lerp_shadows(x, y, t)?.into()),
        (Value::Kw(Kw::None), Value::Shadows(y)) => Value::Shadows(lerp_shadows(&[], y, t)?.into()),
        (Value::Shadows(x), Value::Kw(Kw::None)) => Value::Shadows(lerp_shadows(x, &[], t)?.into()),
        _ => return None,
    })
}

/// True if a transition between the two would show intermediate values.
pub fn can_blend(a: &Value, b: &Value) -> bool {
    blend(a, b, 0.5).is_some()
}

pub fn interpolate(prop: PropId, a: &Value, b: &Value, t: f32) -> Value {
    if let Some(v) = blend(a, b, t) {
        return v;
    }
    if prop == PropId::Visibility {
        // shown for the whole time if either end is shown
        let vis = |v: &Value| matches!(v, Value::Kw(Kw::Visible));
        if t > 0.0 && t < 1.0 {
            return if vis(a) { a.clone() } else { b.clone() };
        }
    }
    if t < 0.5 {
        a.clone()
    } else {
        b.clone()
    }
}

fn size_value(s: &Size) -> Option<Value> {
    Some(match s {
        Size::Px(v) => Value::Len(Len::Px(*v)),
        Size::Pct(v) => Value::Len(Len::Pct(*v)),
        Size::Auto => Value::Kw(Kw::Auto),
        _ => return None,
    })
}

fn size_len(s: &Size) -> Len {
    match s {
        Size::Px(v) => Len::Px(*v),
        Size::Pct(v) => Len::Pct(*v),
        _ => Len::Px(0.0),
    }
}

fn color_value(c: Color) -> Option<Value> {
    Some(Value::Color(CssColor::Rgba(c)))
}

/// The computed value of `prop` in `st`, in the form the cascade accepts back.
pub fn computed_value(st: &ComputedStyle, prop: PropId) -> Option<Value> {
    use PropId::*;
    let px = |v: f32| Some(Value::Len(Len::Px(v)));
    match prop {
        Opacity => Some(Value::Num(st.opacity)),
        Transform => Some(match &st.transform {
            None => Value::Kw(Kw::None),
            Some(ops) => Value::Transform(
                ops.iter()
                    .map(|op| match op {
                        TransformOp::Translate(x, y) => TransformFn::Translate(size_len(x), size_len(y)),
                        TransformOp::Scale(x, y) => TransformFn::Scale(*x, *y),
                        TransformOp::Rotate(d) => TransformFn::Rotate(*d),
                        TransformOp::Skew(x, y) => TransformFn::Skew(*x, *y),
                        TransformOp::Matrix(m) => TransformFn::Matrix(*m),
                    })
                    .collect(),
            ),
        }),
        Translate => Some(match &st.translate {
            None => Value::Translate(Len::Px(0.0), Len::Px(0.0)),
            Some((x, y)) => Value::Translate(size_len(x), size_len(y)),
        }),
        Rotate => Some(Value::Num(st.rotate)),
        Scale => Some(Value::Scale(st.scale.0, st.scale.1)),
        Color => color_value(st.color),
        BackgroundColor => color_value(st.background_color),
        BorderTopColor => color_value(st.border_color[0]),
        BorderRightColor => color_value(st.border_color[1]),
        BorderBottomColor => color_value(st.border_color[2]),
        BorderLeftColor => color_value(st.border_color[3]),
        OutlineColor => color_value(st.outline_color),
        TextFillColor => color_value(st.text_fill.unwrap_or(st.color)),
        Width => size_value(&st.width),
        Height => size_value(&st.height),
        MinWidth => size_value(&st.min_width),
        MinHeight => size_value(&st.min_height),
        MaxWidth => size_value(&st.max_width),
        MaxHeight => size_value(&st.max_height),
        Top => size_value(&st.inset[0]),
        Right => size_value(&st.inset[1]),
        Bottom => size_value(&st.inset[2]),
        Left => size_value(&st.inset[3]),
        MarginTop => size_value(&st.margin[0]),
        MarginRight => size_value(&st.margin[1]),
        MarginBottom => size_value(&st.margin[2]),
        MarginLeft => size_value(&st.margin[3]),
        PaddingTop => size_value(&st.padding[0]),
        PaddingRight => size_value(&st.padding[1]),
        PaddingBottom => size_value(&st.padding[2]),
        PaddingLeft => size_value(&st.padding[3]),
        BorderTopWidth => px(st.border_width[0]),
        BorderRightWidth => px(st.border_width[1]),
        BorderBottomWidth => px(st.border_width[2]),
        BorderLeftWidth => px(st.border_width[3]),
        BorderTopLeftRadius => size_value(&st.border_radius[0]),
        BorderTopRightRadius => size_value(&st.border_radius[1]),
        BorderBottomRightRadius => size_value(&st.border_radius[2]),
        BorderBottomLeftRadius => size_value(&st.border_radius[3]),
        OutlineWidth => px(st.outline_width),
        OutlineOffset => px(st.outline_offset),
        FontSize => px(st.font_size),
        LetterSpacing => px(st.letter_spacing),
        BoxShadow => Some(match &st.box_shadow {
            None => Value::Kw(Kw::None),
            Some(list) => Value::Shadows(
                list.iter()
                    .map(|s| ShadowSpec {
                        x: Len::Px(s.x),
                        y: Len::Px(s.y),
                        blur: Len::Px(s.blur),
                        spread: Len::Px(s.spread),
                        color: Some(s.color),
                        inset: s.inset,
                    })
                    .collect(),
            ),
        }),
        Visibility => Some(Value::Kw(if st.visibility == style::Visibility::Visible { Kw::Visible } else { Kw::Hidden })),
        _ => None,
    }
}
