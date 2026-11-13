//! Property table, keyword table, declaration parsing and shorthand expansion.

use super::tokenizer::{split_commas, Cur, Tok};
use super::values::*;
use crate::anim::Easing;
use crate::atom::Atom;
use std::rc::Rc;

macro_rules! keywords {
    ($($v:ident = $s:literal),* $(,)?) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        #[repr(u8)]
        pub enum Kw { $($v),* }
        impl Kw {
            pub fn from_ident(s: &str) -> Option<Kw> {
                let mut buf = [0u8; 32];
                let b = s.as_bytes();
                if b.len() > 32 { return None; }
                for (i, c) in b.iter().enumerate() { buf[i] = c.to_ascii_lowercase(); }
                let l = std::str::from_utf8(&buf[..b.len()]).ok()?;
                match l { $($s => Some(Kw::$v),)* _ => None }
            }
        }
    };
}

keywords! {
    Inherit = "inherit", Initial = "initial", Unset = "unset", Revert = "revert",
    Auto = "auto", None = "none", Normal = "normal",
    // display
    Block = "block", Inline = "inline", InlineBlock = "inline-block", Flex = "flex",
    InlineFlex = "inline-flex", Grid = "grid", InlineGrid = "inline-grid", Table = "table",
    InlineTable = "inline-table", TableRow = "table-row", TableCell = "table-cell",
    TableRowGroup = "table-row-group", TableHeaderGroup = "table-header-group",
    TableFooterGroup = "table-footer-group", TableColumn = "table-column",
    TableColumnGroup = "table-column-group", TableCaption = "table-caption", ListItem = "list-item",
    Contents = "contents", FlowRoot = "flow-root",
    // position
    Static = "static", Relative = "relative", Absolute = "absolute", Fixed = "fixed", Sticky = "sticky",
    // sides / alignment
    Left = "left", Right = "right", Both = "both", Center = "center", Justify = "justify",
    Start = "start", End = "end", Top = "top", Bottom = "bottom", Middle = "middle",
    InlineStart = "inline-start", InlineEnd = "inline-end",
    // box-sizing
    ContentBox = "content-box", BorderBox = "border-box", PaddingBox = "padding-box",
    // visibility / overflow
    Visible = "visible", Hidden = "hidden", Collapse = "collapse", Scroll = "scroll", Clip = "clip",
    Overlay = "overlay",
    // border styles
    Solid = "solid", Dashed = "dashed", Dotted = "dotted", Double = "double", Groove = "groove",
    Ridge = "ridge", Inset = "inset", Outset = "outset",
    Thin = "thin", Medium = "medium", Thick = "thick",
    // font
    Bold = "bold", Bolder = "bolder", Lighter = "lighter", Italic = "italic", Oblique = "oblique",
    XxSmall = "xx-small", XSmall = "x-small", Small = "small", Large = "large", XLarge = "x-large",
    XxLarge = "xx-large", XxxLarge = "xxx-large", Smaller = "smaller", Larger = "larger",
    // text
    Underline = "underline", Overline = "overline", LineThrough = "line-through",
    Uppercase = "uppercase", Lowercase = "lowercase", Capitalize = "capitalize",
    Nowrap = "nowrap", Pre = "pre", PreWrap = "pre-wrap", PreLine = "pre-line",
    BreakSpaces = "break-spaces", Wrap = "wrap",
    Baseline = "baseline", Sub = "sub", Super = "super", TextTop = "text-top", TextBottom = "text-bottom",
    BreakWord = "break-word", BreakAll = "break-all", Anywhere = "anywhere", KeepAll = "keep-all",
    Ellipsis = "ellipsis",
    // lists
    Disc = "disc", Circle = "circle", Square = "square", Decimal = "decimal",
    DecimalLeadingZero = "decimal-leading-zero", LowerAlpha = "lower-alpha", UpperAlpha = "upper-alpha",
    LowerLatin = "lower-latin", UpperLatin = "upper-latin", LowerRoman = "lower-roman",
    UpperRoman = "upper-roman", Inside = "inside", Outside = "outside",
    // cursor
    Pointer = "pointer", Default = "default", Text = "text", Move = "move", NotAllowed = "not-allowed",
    Grab = "grab", Grabbing = "grabbing", Crosshair = "crosshair", Help = "help", Wait = "wait",
    Progress = "progress", ZoomIn = "zoom-in", ZoomOut = "zoom-out", ColResize = "col-resize",
    RowResize = "row-resize", EwResize = "ew-resize", NsResize = "ns-resize", All = "all",
    // flex / grid
    Row = "row", RowReverse = "row-reverse", Column = "column", ColumnReverse = "column-reverse",
    WrapReverse = "wrap-reverse", FlexStart = "flex-start", FlexEnd = "flex-end",
    SpaceBetween = "space-between", SpaceAround = "space-around", SpaceEvenly = "space-evenly",
    Stretch = "stretch", SelfStart = "self-start", SelfEnd = "self-end", Dense = "dense",
    Content = "content", MinContent = "min-content", MaxContent = "max-content", FitContent = "fit-content",
    Safe = "safe", Unsafe = "unsafe", First = "first", Last = "last",
    // misc
    Separate = "separate", Cover = "cover", Contain = "contain", Fill = "fill", ScaleDown = "scale-down",
    Repeat = "repeat", NoRepeat = "no-repeat", RepeatX = "repeat-x", RepeatY = "repeat-y",
    Space = "space", Round = "round", Local = "local", Transparent = "transparent",
    CurrentColor = "currentcolor", Wavy = "wavy", WebkitCenter = "-webkit-center",
}

macro_rules! props {
    (longhands: { $($l:ident = $ln:literal),* $(,)? } shorthands: { $($s:ident = $sn:literal),* $(,)? }) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        #[repr(u16)]
        pub enum PropId { $($l,)* Custom, $($s,)* }
        fn prop_lookup(n: &str) -> Option<PropId> {
            match n { $($ln => Some(PropId::$l),)* $($sn => Some(PropId::$s),)* _ => None }
        }
        impl PropId {
            pub fn name(self) -> &'static str {
                match self { $(PropId::$l => $ln,)* PropId::Custom => "--", $(PropId::$s => $sn,)* }
            }
        }
    };
}

props! {
    longhands: {
        Display = "display", Position = "position", Float = "float", Clear = "clear",
        BoxSizing = "box-sizing", Visibility = "visibility", Opacity = "opacity", ZIndex = "z-index",
        OverflowX = "overflow-x", OverflowY = "overflow-y",
        Width = "width", Height = "height", MinWidth = "min-width", MinHeight = "min-height",
        MaxWidth = "max-width", MaxHeight = "max-height",
        MarginTop = "margin-top", MarginRight = "margin-right", MarginBottom = "margin-bottom",
        MarginLeft = "margin-left",
        PaddingTop = "padding-top", PaddingRight = "padding-right", PaddingBottom = "padding-bottom",
        PaddingLeft = "padding-left",
        BorderTopWidth = "border-top-width", BorderRightWidth = "border-right-width",
        BorderBottomWidth = "border-bottom-width", BorderLeftWidth = "border-left-width",
        BorderTopStyle = "border-top-style", BorderRightStyle = "border-right-style",
        BorderBottomStyle = "border-bottom-style", BorderLeftStyle = "border-left-style",
        BorderTopColor = "border-top-color", BorderRightColor = "border-right-color",
        BorderBottomColor = "border-bottom-color", BorderLeftColor = "border-left-color",
        BorderTopLeftRadius = "border-top-left-radius", BorderTopRightRadius = "border-top-right-radius",
        BorderBottomRightRadius = "border-bottom-right-radius",
        BorderBottomLeftRadius = "border-bottom-left-radius",
        Top = "top", Right = "right", Bottom = "bottom", Left = "left",
        Color = "color", BackgroundColor = "background-color", BackgroundImage = "background-image",
        BackgroundSize = "background-size", BackgroundPosition = "background-position",
        BackgroundRepeat = "background-repeat",
        FontFamily = "font-family", FontSize = "font-size", FontWeight = "font-weight",
        FontStyle = "font-style", LineHeight = "line-height", TextAlign = "text-align",
        TextDecorationLine = "text-decoration-line", TextDecorationColor = "text-decoration-color",
        TextTransform = "text-transform", TextIndent = "text-indent", LetterSpacing = "letter-spacing",
        WordSpacing = "word-spacing", WhiteSpace = "white-space", VerticalAlign = "vertical-align",
        WordBreak = "word-break", OverflowWrap = "overflow-wrap", TextOverflow = "text-overflow",
        ListStyleType = "list-style-type", Cursor = "cursor", PointerEvents = "pointer-events",
        FlexDirection = "flex-direction", FlexWrap = "flex-wrap", JustifyContent = "justify-content",
        AlignItems = "align-items", AlignSelf = "align-self", AlignContent = "align-content",
        JustifyItems = "justify-items", JustifySelf = "justify-self",
        FlexGrow = "flex-grow", FlexShrink = "flex-shrink", FlexBasis = "flex-basis", Order = "order",
        RowGap = "row-gap", ColumnGap = "column-gap",
        GridTemplateColumns = "grid-template-columns", GridTemplateRows = "grid-template-rows",
        GridColumnStart = "grid-column-start", GridColumnEnd = "grid-column-end",
        GridRowStart = "grid-row-start", GridRowEnd = "grid-row-end", GridAutoFlow = "grid-auto-flow",
        GridAutoRows = "grid-auto-rows", GridAutoColumns = "grid-auto-columns",
        GridTemplateAreas = "grid-template-areas", GridAreaName = "-invis-grid-area-name",
        MaskImage = "mask-image",
        BoxShadow = "box-shadow", BorderCollapse = "border-collapse", BorderSpacing = "border-spacing",
        TableLayout = "table-layout", Content = "content", ObjectFit = "object-fit",
        AspectRatio = "aspect-ratio", Transform = "transform", Translate = "translate",
        Rotate = "rotate", Scale = "scale", TransformOrigin = "transform-origin",
        TransitionProperty = "transition-property", TransitionDuration = "transition-duration",
        TransitionTimingFunction = "transition-timing-function", TransitionDelay = "transition-delay",
        AnimationName = "animation-name", AnimationDuration = "animation-duration",
        AnimationTimingFunction = "animation-timing-function", AnimationDelay = "animation-delay",
        AnimationIterationCount = "animation-iteration-count", AnimationDirection = "animation-direction",
        AnimationFillMode = "animation-fill-mode", AnimationPlayState = "animation-play-state",
        BackgroundClip = "background-clip", TextFillColor = "-webkit-text-fill-color",
        OutlineWidth = "outline-width", OutlineStyle = "outline-style", OutlineColor = "outline-color",
        OutlineOffset = "outline-offset",
    }
    shorthands: {
        Margin = "margin", Padding = "padding", Inset = "inset",
        Border = "border", BorderTop = "border-top", BorderRight = "border-right",
        BorderBottom = "border-bottom", BorderLeft = "border-left",
        BorderWidth = "border-width", BorderStyle = "border-style", BorderColor = "border-color",
        BorderRadius = "border-radius", Background = "background", Font = "font", Flex = "flex",
        FlexFlow = "flex-flow", Gap = "gap", Overflow = "overflow", ListStyle = "list-style",
        TextDecoration = "text-decoration", Outline = "outline", GridColumn = "grid-column",
        GridRow = "grid-row", GridArea = "grid-area", GridTemplate = "grid-template",
        PlaceItems = "place-items", PlaceContent = "place-content", PlaceSelf = "place-self",
        MarginInline = "margin-inline", MarginBlock = "margin-block",
        PaddingInline = "padding-inline", PaddingBlock = "padding-block",
        InsetInline = "inset-inline", InsetBlock = "inset-block",
        BorderInline = "border-inline", BorderBlock = "border-block",
        Transition = "transition", Animation = "animation",
    }
}

impl PropId {
    pub fn is_shorthand(self) -> bool {
        (self as u16) > (PropId::Custom as u16)
    }
}

/// Map a property name (lower-case) to its id, resolving logical and vendor-prefixed aliases.
pub fn prop_from_name(name: &str) -> Option<PropId> {
    if let Some(p) = prop_lookup(name) {
        return Some(p);
    }
    let alias = match name {
        "margin-inline-start" => "margin-left",
        "margin-inline-end" => "margin-right",
        "margin-block-start" => "margin-top",
        "margin-block-end" => "margin-bottom",
        "padding-inline-start" => "padding-left",
        "padding-inline-end" => "padding-right",
        "padding-block-start" => "padding-top",
        "padding-block-end" => "padding-bottom",
        "inset-inline-start" => "left",
        "inset-inline-end" => "right",
        "inset-block-start" => "top",
        "inset-block-end" => "bottom",
        "inline-size" => "width",
        "block-size" => "height",
        "min-inline-size" => "min-width",
        "min-block-size" => "min-height",
        "max-inline-size" => "max-width",
        "max-block-size" => "max-height",
        "border-inline-start" => "border-left",
        "border-inline-end" => "border-right",
        "border-block-start" => "border-top",
        "border-block-end" => "border-bottom",
        "border-inline-start-width" => "border-left-width",
        "border-inline-end-width" => "border-right-width",
        "border-block-start-width" => "border-top-width",
        "border-block-end-width" => "border-bottom-width",
        "border-inline-start-color" => "border-left-color",
        "border-inline-end-color" => "border-right-color",
        "border-block-start-color" => "border-top-color",
        "border-block-end-color" => "border-bottom-color",
        "border-inline-start-style" => "border-left-style",
        "border-inline-end-style" => "border-right-style",
        "border-block-start-style" => "border-top-style",
        "border-block-end-style" => "border-bottom-style",
        "border-start-start-radius" => "border-top-left-radius",
        "border-start-end-radius" => "border-top-right-radius",
        "border-end-start-radius" => "border-bottom-left-radius",
        "border-end-end-radius" => "border-bottom-right-radius",
        "grid-gap" => "gap",
        "grid-row-gap" => "row-gap",
        "grid-column-gap" => "column-gap",
        "word-wrap" => "overflow-wrap",
        "overflow-inline" => "overflow-x",
        "overflow-block" => "overflow-y",
        "mask" | "-webkit-mask" | "-webkit-mask-image" => "mask-image",
        "grid" => "grid-template",
        _ => {
            let stripped = name
                .strip_prefix("-webkit-")
                .or_else(|| name.strip_prefix("-moz-"))
                .or_else(|| name.strip_prefix("-ms-"))?;
            return match stripped {
                "box-shadow" | "border-radius" | "box-sizing" | "transform" | "flex" | "flex-direction"
                | "flex-wrap" | "flex-flow" | "flex-grow" | "flex-shrink" | "flex-basis" | "order"
                | "justify-content" | "align-items" | "align-self" | "align-content"
                | "border-top-left-radius" | "border-top-right-radius" | "border-bottom-left-radius"
                | "border-bottom-right-radius" | "background-size" | "background-clip" | "transform-origin"
                | "transition" | "transition-property" | "transition-duration" | "transition-timing-function"
                | "transition-delay" | "animation" | "animation-name" | "animation-duration"
                | "animation-timing-function" | "animation-delay" | "animation-iteration-count"
                | "animation-direction" | "animation-fill-mode" | "animation-play-state" => prop_lookup(stripped),
                _ => None,
            };
        }
    };
    prop_lookup(alias)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Kw(Kw),
    Len(Len),
    Num(f32),
    Int(i32),
    Color(CssColor),
    Families(Rc<[Atom]>),
    Images(Rc<[Image]>),
    BgSizes(Rc<[BgSize]>),
    Positions(Rc<[(Len, Len)]>),
    Repeats(Rc<[(bool, bool)]>),
    Shadows(Rc<[ShadowSpec]>),
    Tracks(Rc<[TrackItem]>),
    GridLine(GridLine),
    Pair(Len, Len),
    Content(Rc<[ContentItem]>),
    Translate(Len, Len),
    /// A `transform` function list.
    Transform(Rc<[TransformFn]>),
    /// The standalone `scale` property.
    Scale(f32, f32),
    /// A list of times in milliseconds (durations, delays).
    Times(Rc<[f32]>),
    Easings(Rc<[Easing]>),
    /// `transition-property`.
    TransProps(Rc<[TransProp]>),
    /// `animation-name` (`Atom::NONE` for `none`).
    Names(Rc<[Atom]>),
    /// A list of numbers: iteration counts (infinity for `infinite`), or codes for the
    /// direction, fill mode and play state keywords (see `anim_code`).
    Nums(Rc<[f32]>),
    Areas(Rc<GridAreas>),
    /// A plain identifier (e.g. a grid area name).
    Name(Atom),
    /// A custom property: name (with the leading `--`) and its raw text.
    Custom(Atom, Rc<str>),
    /// A value containing `var()`: resolved and parsed at cascade time.
    Pending(Rc<str>),
}

/// One entry of `transition-property`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransProp {
    All,
    None,
    /// A property (possibly a shorthand) this engine knows.
    Prop(PropId),
    /// Something else: keeps its place in the list, never transitions.
    Other,
}

#[derive(Clone, Debug)]
pub struct Declaration {
    pub prop: PropId,
    pub value: Value,
    pub important: bool,
}

pub type UrlResolver<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn longhands(sh: PropId) -> &'static [PropId] {
    use PropId::*;
    match sh {
        Margin => &[MarginTop, MarginRight, MarginBottom, MarginLeft],
        Padding => &[PaddingTop, PaddingRight, PaddingBottom, PaddingLeft],
        Inset => &[Top, Right, Bottom, Left],
        Border => &[
            BorderTopWidth, BorderRightWidth, BorderBottomWidth, BorderLeftWidth, BorderTopStyle,
            BorderRightStyle, BorderBottomStyle, BorderLeftStyle, BorderTopColor, BorderRightColor,
            BorderBottomColor, BorderLeftColor,
        ],
        BorderTop => &[BorderTopWidth, BorderTopStyle, BorderTopColor],
        BorderRight => &[BorderRightWidth, BorderRightStyle, BorderRightColor],
        BorderBottom => &[BorderBottomWidth, BorderBottomStyle, BorderBottomColor],
        BorderLeft => &[BorderLeftWidth, BorderLeftStyle, BorderLeftColor],
        BorderInline => &[
            BorderLeftWidth, BorderLeftStyle, BorderLeftColor, BorderRightWidth, BorderRightStyle,
            BorderRightColor,
        ],
        BorderBlock => &[
            BorderTopWidth, BorderTopStyle, BorderTopColor, BorderBottomWidth, BorderBottomStyle,
            BorderBottomColor,
        ],
        BorderWidth => &[BorderTopWidth, BorderRightWidth, BorderBottomWidth, BorderLeftWidth],
        BorderStyle => &[BorderTopStyle, BorderRightStyle, BorderBottomStyle, BorderLeftStyle],
        BorderColor => &[BorderTopColor, BorderRightColor, BorderBottomColor, BorderLeftColor],
        BorderRadius => &[BorderTopLeftRadius, BorderTopRightRadius, BorderBottomRightRadius, BorderBottomLeftRadius],
        Background => &[BackgroundColor, BackgroundImage, BackgroundSize, BackgroundPosition, BackgroundRepeat, BackgroundClip],
        Font => &[FontStyle, FontWeight, FontSize, LineHeight, FontFamily],
        Flex => &[FlexGrow, FlexShrink, FlexBasis],
        FlexFlow => &[FlexDirection, FlexWrap],
        Gap => &[RowGap, ColumnGap],
        Overflow => &[OverflowX, OverflowY],
        ListStyle => &[ListStyleType],
        TextDecoration => &[TextDecorationLine, TextDecorationColor],
        Outline => &[OutlineWidth, OutlineStyle, OutlineColor],
        GridColumn => &[GridColumnStart, GridColumnEnd],
        GridRow => &[GridRowStart, GridRowEnd],
        GridArea => &[GridRowStart, GridColumnStart, GridRowEnd, GridColumnEnd, GridAreaName],
        GridTemplate => &[GridTemplateRows, GridTemplateColumns, GridTemplateAreas],
        PlaceItems => &[AlignItems, JustifyItems],
        PlaceContent => &[AlignContent, JustifyContent],
        PlaceSelf => &[AlignSelf, JustifySelf],
        MarginInline => &[MarginLeft, MarginRight],
        MarginBlock => &[MarginTop, MarginBottom],
        PaddingInline => &[PaddingLeft, PaddingRight],
        PaddingBlock => &[PaddingTop, PaddingBottom],
        InsetInline => &[Left, Right],
        InsetBlock => &[Top, Bottom],
        Transition => &[TransitionProperty, TransitionDuration, TransitionTimingFunction, TransitionDelay],
        Animation => &[
            AnimationName, AnimationDuration, AnimationTimingFunction, AnimationDelay, AnimationIterationCount,
            AnimationDirection, AnimationFillMode, AnimationPlayState,
        ],
        _ => &[],
    }
}

fn has_var(toks: &[Tok]) -> bool {
    toks.iter().any(|t| matches!(t, Tok::Function(f) if f.eq_ignore_ascii_case("var") || f.eq_ignore_ascii_case("env")))
}

fn trim(toks: &[Tok]) -> &[Tok] {
    let mut s = 0;
    let mut e = toks.len();
    while s < e && matches!(toks[s], Tok::Whitespace) {
        s += 1;
    }
    while e > s && matches!(toks[e - 1], Tok::Whitespace) {
        e -= 1;
    }
    &toks[s..e]
}

/// Parse one declaration and append the resulting longhand declarations to `out`.
pub fn parse_declaration(
    name: &str,
    toks: &[Tok],
    raw: &str,
    important: bool,
    resolve: UrlResolver,
    out: &mut Vec<Declaration>,
) {
    if name.starts_with("--") {
        out.push(Declaration {
            prop: PropId::Custom,
            value: Value::Custom(Atom::new(name), raw.trim().into()),
            important,
        });
        return;
    }
    let lname = name.to_ascii_lowercase();
    let Some(prop) = prop_from_name(&lname) else { return };
    let toks = trim(toks);
    if toks.is_empty() {
        return;
    }
    if has_var(toks) {
        out.push(Declaration { prop, value: Value::Pending(raw.trim().into()), important });
        return;
    }
    parse_value(prop, toks, important, resolve, out);
}

/// Parse the value tokens of `prop` (which contain no `var()`), expanding shorthands.
pub fn parse_value(prop: PropId, toks: &[Tok], important: bool, resolve: UrlResolver, out: &mut Vec<Declaration>) {
    let toks = trim(toks);
    // CSS-wide keywords
    if toks.len() == 1 {
        if let Tok::Ident(i) = &toks[0] {
            if let Some(kw @ (Kw::Inherit | Kw::Initial | Kw::Unset | Kw::Revert)) = Kw::from_ident(i) {
                if prop.is_shorthand() {
                    for &l in longhands(prop) {
                        out.push(Declaration { prop: l, value: Value::Kw(kw), important });
                    }
                } else {
                    out.push(Declaration { prop, value: Value::Kw(kw), important });
                }
                return;
            }
        }
    }
    if prop.is_shorthand() {
        let start = out.len();
        if expand_shorthand(prop, toks, resolve, out).is_none() {
            out.truncate(start);
        } else if important {
            for d in &mut out[start..] {
                d.important = true;
            }
        }
    } else if let Some(v) = parse_longhand(prop, toks, resolve) {
        out.push(Declaration { prop, value: v, important });
    }
}

fn single_kw(toks: &[Tok]) -> Option<Kw> {
    match toks {
        [Tok::Ident(i)] => Kw::from_ident(i),
        _ => None,
    }
}

fn kw_in(toks: &[Tok], allowed: &[Kw]) -> Option<Value> {
    let k = single_kw(toks)?;
    if allowed.contains(&k) {
        Some(Value::Kw(k))
    } else {
        None
    }
}

fn len_only(toks: &[Tok]) -> Option<Len> {
    let mut c = Cur::new(toks);
    let l = parse_len(&mut c, false)?;
    if c.at_end() {
        Some(l)
    } else {
        None
    }
}

fn len_or_kw(toks: &[Tok], allowed: &[Kw]) -> Option<Value> {
    if let Some(v) = kw_in(toks, allowed) {
        return Some(v);
    }
    len_only(toks).map(Value::Len)
}

fn color_only(toks: &[Tok]) -> Option<Value> {
    let mut c = Cur::new(toks);
    let col = parse_color(&mut c)?;
    if c.at_end() {
        Some(Value::Color(col))
    } else {
        None
    }
}

const BORDER_STYLES: &[Kw] = &[
    Kw::None, Kw::Hidden, Kw::Solid, Kw::Dashed, Kw::Dotted, Kw::Double, Kw::Groove, Kw::Ridge, Kw::Inset,
    Kw::Outset,
];
const ALIGN_KWS: &[Kw] = &[
    Kw::Normal, Kw::Auto, Kw::Stretch, Kw::Center, Kw::Start, Kw::End, Kw::FlexStart, Kw::FlexEnd,
    Kw::SelfStart, Kw::SelfEnd, Kw::Baseline, Kw::SpaceBetween, Kw::SpaceAround, Kw::SpaceEvenly, Kw::Left,
    Kw::Right,
];
const SIZE_KWS: &[Kw] = &[Kw::Auto, Kw::MinContent, Kw::MaxContent, Kw::FitContent];
const MAX_SIZE_KWS: &[Kw] = &[Kw::None, Kw::MinContent, Kw::MaxContent, Kw::FitContent];

fn parse_align(toks: &[Tok]) -> Option<Value> {
    // drop `safe` / `unsafe` / `first` / `last` modifiers
    let filtered: Vec<Tok> = toks
        .iter()
        .filter(|t| match t {
            Tok::Whitespace => false,
            Tok::Ident(i) => !matches!(Kw::from_ident(i), Some(Kw::Safe | Kw::Unsafe | Kw::First | Kw::Last)),
            _ => true,
        })
        .cloned()
        .collect();
    kw_in(&filtered, ALIGN_KWS)
}

fn parse_size(toks: &[Tok], kws: &[Kw]) -> Option<Value> {
    if let Some(v) = kw_in(toks, kws) {
        return Some(v);
    }
    if let [Tok::Function(f), ..] = toks {
        if f.eq_ignore_ascii_case("fit-content") {
            return Some(Value::Kw(Kw::FitContent));
        }
    }
    if let [Tok::Ident(i)] = toks {
        // -webkit-fill-available / stretch behave like auto for block boxes
        let l = i.to_ascii_lowercase();
        if l.ends_with("fill-available") || l == "stretch" || l == "-moz-available" {
            return Some(Value::Kw(Kw::Auto));
        }
    }
    let l = len_only(toks)?;
    Some(Value::Len(l))
}

fn parse_border_width_tok(c: &mut Cur) -> Option<Value> {
    let save = c.i;
    if let Some(i) = c.ident() {
        match Kw::from_ident(i) {
            Some(Kw::Thin) => return Some(Value::Len(Len::Px(1.0))),
            Some(Kw::Medium) => return Some(Value::Len(Len::Px(3.0))),
            Some(Kw::Thick) => return Some(Value::Len(Len::Px(5.0))),
            _ => {
                c.i = save;
                return None;
            }
        }
    }
    parse_len(c, false).map(Value::Len)
}

fn parse_families(toks: &[Tok]) -> Option<Value> {
    let mut out: Vec<Atom> = Vec::new();
    for part in split_commas(toks) {
        let part = trim(part);
        match part {
            [Tok::Str(s)] => out.push(Atom::new(&s.to_ascii_lowercase())),
            _ => {
                let mut name = String::new();
                for t in part {
                    match t {
                        Tok::Ident(i) => {
                            if !name.is_empty() {
                                name.push(' ');
                            }
                            name.push_str(&i.to_ascii_lowercase());
                        }
                        Tok::Whitespace => {}
                        _ => return None,
                    }
                }
                if name.is_empty() {
                    return None;
                }
                out.push(Atom::new(&name));
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Families(out.into()))
    }
}

fn parse_font_weight(toks: &[Tok]) -> Option<Value> {
    match toks {
        [Tok::Number(n)] if *n >= 1.0 && *n <= 1000.0 => Some(Value::Num(*n)),
        _ => match single_kw(toks)? {
            Kw::Normal => Some(Value::Num(400.0)),
            Kw::Bold => Some(Value::Num(700.0)),
            k @ (Kw::Bolder | Kw::Lighter) => Some(Value::Kw(k)),
            _ => None,
        },
    }
}

fn parse_font_size(toks: &[Tok]) -> Option<Value> {
    if let Some(v) = kw_in(
        toks,
        &[
            Kw::XxSmall, Kw::XSmall, Kw::Small, Kw::Medium, Kw::Large, Kw::XLarge, Kw::XxLarge, Kw::XxxLarge,
            Kw::Smaller, Kw::Larger,
        ],
    ) {
        return Some(v);
    }
    len_only(toks).map(Value::Len)
}

/// The value of a `calc()` tree that contains only numbers; `None` if a length is involved.
fn calc_number(c: &Calc) -> Option<f32> {
    Some(match c {
        Calc::Num(n) => *n,
        Calc::Leaf(_) => return None,
        Calc::Add(a, b) => calc_number(a)? + calc_number(b)?,
        Calc::Sub(a, b) => calc_number(a)? - calc_number(b)?,
        Calc::Mul(a, b) => calc_number(a)? * calc_number(b)?,
        Calc::Div(a, b) => {
            let d = calc_number(b)?;
            if d == 0.0 {
                return None;
            }
            calc_number(a)? / d
        }
        Calc::Min(list) => list.iter().map(calc_number).collect::<Option<Vec<f32>>>()?.into_iter().fold(f32::INFINITY, f32::min),
        Calc::Max(list) => list.iter().map(calc_number).collect::<Option<Vec<f32>>>()?.into_iter().fold(f32::NEG_INFINITY, f32::max),
        Calc::Clamp(lo, v, hi) => calc_number(v)?.max(calc_number(lo)?).min(calc_number(hi)?),
        Calc::Round(mode, v, step) => mode.apply(calc_number(v)?, calc_number(step)?),
    })
}

fn parse_line_height(toks: &[Tok]) -> Option<Value> {
    match toks {
        [Tok::Number(n)] if *n >= 0.0 => Some(Value::Num(*n)),
        _ => {
            // `calc()` over plain numbers is a multiplier, not a length: utility frameworks
            // write `line-height: calc(2 / 1.5)`
            let mut c = Cur::new(toks);
            c.skip_ws();
            if let Some(Tok::Function(f)) = c.peek() {
                if f.eq_ignore_ascii_case("calc") {
                    c.next();
                    let mut inner = c.block();
                    if let (Some(n), true) = (parse_calc_sum(&mut inner).and_then(|v| calc_number(&v)), c.at_end()) {
                        return (n >= 0.0).then_some(Value::Num(n));
                    }
                }
            }
            len_or_kw(toks, &[Kw::Normal])
        }
    }
}

fn parse_decoration_line(toks: &[Tok]) -> Option<Value> {
    let mut bits = 0;
    let mut any = false;
    for t in toks {
        match t {
            Tok::Whitespace => {}
            Tok::Ident(i) => {
                match Kw::from_ident(i)? {
                    Kw::None => {}
                    Kw::Underline => bits |= 1,
                    Kw::Overline => bits |= 2,
                    Kw::LineThrough => bits |= 4,
                    _ => return None,
                }
                any = true;
            }
            _ => return None,
        }
    }
    if any {
        Some(Value::Int(bits))
    } else {
        None
    }
}

fn parse_content(toks: &[Tok]) -> Option<Value> {
    if let Some(v) = kw_in(toks, &[Kw::Normal, Kw::None]) {
        return Some(v);
    }
    let mut items = Vec::new();
    let mut c = Cur::new(toks);
    loop {
        c.skip_ws();
        let Some(t) = c.next() else { break };
        match t {
            Tok::Str(s) => items.push(ContentItem::Str(s.as_str().into())),
            Tok::Ident(i) if i.eq_ignore_ascii_case("open-quote") => items.push(ContentItem::OpenQuote),
            Tok::Ident(i) if i.eq_ignore_ascii_case("close-quote") => items.push(ContentItem::CloseQuote),
            Tok::Ident(i) if i.eq_ignore_ascii_case("no-open-quote") || i.eq_ignore_ascii_case("no-close-quote") => {}
            Tok::Function(f) if f.eq_ignore_ascii_case("attr") => {
                let mut inner = c.block();
                let name = inner.ident()?;
                items.push(ContentItem::Attr(Atom::lower(name)));
            }
            Tok::Function(_) => {
                c.block(); // counter(), url(): not rendered
            }
            Tok::Url(_) => {}
            Tok::Delim('/') => break, // alt text follows
            _ => return None,
        }
    }
    Some(Value::Content(items.into()))
}

/// A time in milliseconds (`0.3s`, `150ms`).
pub fn parse_time(c: &mut Cur) -> Option<f32> {
    let save = c.i;
    c.skip_ws();
    let r = match c.next() {
        Some(Tok::Dimension(v, u)) if u.eq_ignore_ascii_case("s") => Some(*v * 1000.0),
        Some(Tok::Dimension(v, u)) if u.eq_ignore_ascii_case("ms") => Some(*v),
        Some(Tok::Function(f)) if f.eq_ignore_ascii_case("calc") => {
            // calc(0.2s * 2) and the like: one time, scaled by plain numbers
            let mut inner = c.block();
            let mut t = parse_time(&mut inner)?;
            loop {
                if inner.delim('*') {
                    t *= parse_number(&mut inner)?;
                } else if inner.delim('/') {
                    let d = parse_number(&mut inner)?;
                    if d == 0.0 {
                        return None;
                    }
                    t /= d;
                } else {
                    break;
                }
            }
            inner.at_end().then_some(t)
        }
        _ => None,
    };
    if r.is_none() {
        c.i = save;
    }
    r.filter(|v| v.is_finite())
}

/// Codes for the animation keywords stored in `Value::Nums`.
pub mod anim_code {
    pub const NORMAL: f32 = 0.0;
    pub const REVERSE: f32 = 1.0;
    pub const ALTERNATE: f32 = 2.0;
    pub const ALTERNATE_REVERSE: f32 = 3.0;
    pub const FILL_NONE: f32 = 0.0;
    pub const FORWARDS: f32 = 1.0;
    pub const BACKWARDS: f32 = 2.0;
    pub const BOTH: f32 = 3.0;
    pub const RUNNING: f32 = 0.0;
    pub const PAUSED: f32 = 1.0;
}

fn direction_code(i: &str) -> Option<f32> {
    Some(match i {
        "normal" => anim_code::NORMAL,
        "reverse" => anim_code::REVERSE,
        "alternate" => anim_code::ALTERNATE,
        "alternate-reverse" => anim_code::ALTERNATE_REVERSE,
        _ => return None,
    })
}

fn fill_code(i: &str) -> Option<f32> {
    Some(match i {
        "none" => anim_code::FILL_NONE,
        "forwards" => anim_code::FORWARDS,
        "backwards" => anim_code::BACKWARDS,
        "both" => anim_code::BOTH,
        _ => return None,
    })
}

fn play_code(i: &str) -> Option<f32> {
    Some(match i {
        "running" => anim_code::RUNNING,
        "paused" => anim_code::PAUSED,
        _ => return None,
    })
}

fn trans_prop(name: &str) -> TransProp {
    let l = name.to_ascii_lowercase();
    match l.as_str() {
        "all" => TransProp::All,
        "none" => TransProp::None,
        _ => match prop_from_name(&l) {
            Some(p) => TransProp::Prop(p),
            None => TransProp::Other,
        },
    }
}

/// A comma-separated list where every item is parsed by `one` and must use up its tokens.
fn comma_list<T>(toks: &[Tok], one: impl Fn(&mut Cur) -> Option<T>) -> Option<Vec<T>> {
    let mut out = Vec::new();
    for part in split_commas(toks) {
        let mut c = Cur::new(part);
        let v = one(&mut c)?;
        if !c.at_end() {
            return None;
        }
        out.push(v);
    }
    (!out.is_empty()).then_some(out)
}

fn ident_code_list(toks: &[Tok], code: fn(&str) -> Option<f32>) -> Option<Value> {
    comma_list(toks, |c| code(&c.ident()?.to_ascii_lowercase())).map(|v| Value::Nums(v.into()))
}

/// `transition`: per item a property, a duration, a timing function and a delay, in any order.
fn expand_transition(toks: &[Tok], out: &mut Vec<Declaration>) -> Option<()> {
    let (mut props, mut durs, mut eases, mut delays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for part in split_commas(toks) {
        let mut c = Cur::new(part);
        let (mut prop, mut times, mut ease): (Option<TransProp>, Vec<f32>, Option<Easing>) = (None, Vec::new(), None);
        while !c.at_end() {
            if let Some(t) = parse_time(&mut c) {
                times.push(t);
                continue;
            }
            let save = c.i;
            if ease.is_none() {
                if let Some(e) = Easing::parse(&mut c) {
                    ease = Some(e);
                    continue;
                }
                c.i = save;
            }
            let name = c.ident()?;
            if prop.is_some() {
                return None;
            }
            prop = Some(trans_prop(name));
        }
        if times.len() > 2 {
            return None;
        }
        props.push(prop.unwrap_or(TransProp::All));
        durs.push(times.first().copied().unwrap_or(0.0).max(0.0));
        delays.push(times.get(1).copied().unwrap_or(0.0));
        eases.push(ease.unwrap_or(Easing::EASE));
    }
    if props.is_empty() {
        return None;
    }
    let mut push = |prop, value| out.push(Declaration { prop, value, important: false });
    push(PropId::TransitionProperty, Value::TransProps(props.into()));
    push(PropId::TransitionDuration, Value::Times(durs.into()));
    push(PropId::TransitionTimingFunction, Value::Easings(eases.into()));
    push(PropId::TransitionDelay, Value::Times(delays.into()));
    Some(())
}

/// `animation`: per item a name plus timing words in any order.
fn expand_animation(toks: &[Tok], out: &mut Vec<Declaration>) -> Option<()> {
    let mut names = Vec::new();
    let (mut durs, mut delays, mut eases) = (Vec::new(), Vec::new(), Vec::new());
    let (mut iters, mut dirs, mut fills, mut plays) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for part in split_commas(toks) {
        let mut c = Cur::new(part);
        let mut name: Option<Atom> = None;
        let (mut times, mut ease): (Vec<f32>, Option<Easing>) = (Vec::new(), None);
        let (mut iter, mut dir, mut fill, mut play): (Option<f32>, Option<f32>, Option<f32>, Option<f32>) = (None, None, None, None);
        while !c.at_end() {
            if let Some(t) = parse_time(&mut c) {
                times.push(t);
                continue;
            }
            c.skip_ws();
            match c.peek() {
                Some(Tok::Number(n)) if iter.is_none() => {
                    iter = Some(n.max(0.0));
                    c.next();
                    continue;
                }
                Some(Tok::Str(s)) if name.is_none() => {
                    name = Some(Atom::new(s));
                    c.next();
                    continue;
                }
                _ => {}
            }
            let save = c.i;
            if ease.is_none() {
                if let Some(e) = Easing::parse(&mut c) {
                    ease = Some(e);
                    continue;
                }
                c.i = save;
            }
            let word = c.ident()?;
            let l = word.to_ascii_lowercase();
            if iter.is_none() && l == "infinite" {
                iter = Some(f32::INFINITY);
            } else if dir.is_none() && direction_code(&l).is_some() {
                dir = direction_code(&l);
            } else if fill.is_none() && fill_code(&l).is_some() {
                fill = fill_code(&l);
            } else if play.is_none() && play_code(&l).is_some() {
                play = play_code(&l);
            } else if name.is_none() {
                name = Some(if l == "none" { Atom::NONE } else { Atom::new(word) });
            } else {
                return None;
            }
        }
        if times.len() > 2 {
            return None;
        }
        names.push(name.unwrap_or(Atom::NONE));
        durs.push(times.first().copied().unwrap_or(0.0).max(0.0));
        delays.push(times.get(1).copied().unwrap_or(0.0));
        eases.push(ease.unwrap_or(Easing::EASE));
        iters.push(iter.unwrap_or(1.0));
        dirs.push(dir.unwrap_or(anim_code::NORMAL));
        fills.push(fill.unwrap_or(anim_code::FILL_NONE));
        plays.push(play.unwrap_or(anim_code::RUNNING));
    }
    if names.is_empty() {
        return None;
    }
    let mut push = |prop, value| out.push(Declaration { prop, value, important: false });
    push(PropId::AnimationName, Value::Names(names.into()));
    push(PropId::AnimationDuration, Value::Times(durs.into()));
    push(PropId::AnimationTimingFunction, Value::Easings(eases.into()));
    push(PropId::AnimationDelay, Value::Times(delays.into()));
    push(PropId::AnimationIterationCount, Value::Nums(iters.into()));
    push(PropId::AnimationDirection, Value::Nums(dirs.into()));
    push(PropId::AnimationFillMode, Value::Nums(fills.into()));
    push(PropId::AnimationPlayState, Value::Nums(plays.into()));
    Some(())
}

/// The standalone `translate` property: `none`, or one to three lengths (the third, z, is ignored).
fn parse_translate_prop(toks: &[Tok]) -> Option<Value> {
    if single_kw(toks) == Some(Kw::None) {
        return Some(Value::Translate(Len::Px(0.0), Len::Px(0.0)));
    }
    let mut c = Cur::new(toks);
    let x = parse_len(&mut c, true)?;
    let y = if c.at_end() { Len::Px(0.0) } else { parse_len(&mut c, true)? };
    if !c.at_end() {
        parse_len(&mut c, true)?;
        if !c.at_end() {
            return None;
        }
    }
    Some(Value::Translate(x, y))
}

fn parse_transform(toks: &[Tok]) -> Option<Value> {
    if kw_in(toks, &[Kw::None]).is_some() {
        return Some(Value::Kw(Kw::None));
    }
    let list = parse_transform_list(toks)?;
    if list.is_empty() && !Cur::new(toks).at_end() {
        // only functions that have no effect on the page (translateZ, perspective)
        return Some(Value::Kw(Kw::None));
    }
    if list.is_empty() {
        return None;
    }
    Some(Value::Transform(list.into()))
}

/// The standalone `rotate` property: `none`, an angle, or an axis and an angle (only rotation
/// in the plane of the page is applied).
fn parse_rotate_prop(toks: &[Tok]) -> Option<Value> {
    if single_kw(toks) == Some(Kw::None) {
        return Some(Value::Num(0.0));
    }
    let mut c = Cur::new(toks);
    if let Some(a) = parse_angle(&mut c) {
        if c.at_end() {
            return Some(Value::Num(a));
        }
    }
    let mut c = Cur::new(toks);
    let mut axis: Option<[f32; 3]> = None;
    let mut angle: Option<f32> = None;
    while !c.at_end() {
        if let Some(a) = parse_angle(&mut c) {
            angle = Some(a);
        } else if let Some(i) = c.ident() {
            axis = Some(match i.to_ascii_lowercase().as_str() {
                "x" => [1.0, 0.0, 0.0],
                "y" => [0.0, 1.0, 0.0],
                "z" => [0.0, 0.0, 1.0],
                _ => return None,
            });
        } else {
            let x = parse_number(&mut c)?;
            let y = parse_number(&mut c)?;
            let z = parse_number(&mut c)?;
            axis = Some([x, y, z]);
        }
    }
    let angle = angle?;
    match axis {
        None => Some(Value::Num(angle)),
        Some([x, y, z]) if x == 0.0 && y == 0.0 && z != 0.0 => Some(Value::Num(if z < 0.0 { -angle } else { angle })),
        Some(_) => Some(Value::Num(0.0)),
    }
}

/// The standalone `scale` property: `none`, or one to three factors (the third, z, is ignored).
fn parse_scale_prop(toks: &[Tok]) -> Option<Value> {
    if single_kw(toks) == Some(Kw::None) {
        return Some(Value::Scale(1.0, 1.0));
    }
    let mut c = Cur::new(toks);
    let x = parse_factor(&mut c)?;
    let y = if c.at_end() { x } else { parse_factor(&mut c)? };
    if !c.at_end() {
        parse_factor(&mut c)?;
        if !c.at_end() {
            return None;
        }
    }
    (x.is_finite() && y.is_finite()).then_some(Value::Scale(x, y))
}

/// `transform-origin`: one or two of left/center/right/top/bottom or lengths (a third value,
/// the z offset, is ignored).
fn parse_transform_origin(toks: &[Tok]) -> Option<Value> {
    let mut c = Cur::new(toks);
    let (mut x, mut y): (Option<Len>, Option<Len>) = (None, None);
    let mut n = 0;
    while !c.at_end() && n < 3 {
        n += 1;
        if let Some(i) = c.ident() {
            match i.to_ascii_lowercase().as_str() {
                "left" => x = Some(Len::Pct(0.0)),
                "right" => x = Some(Len::Pct(100.0)),
                "top" => y = Some(Len::Pct(0.0)),
                "bottom" => y = Some(Len::Pct(100.0)),
                "center" => {
                    if x.is_none() && n == 1 {
                        x = Some(Len::Pct(50.0));
                    } else if y.is_none() {
                        y = Some(Len::Pct(50.0));
                    } else {
                        x.get_or_insert(Len::Pct(50.0));
                    }
                }
                _ => return None,
            }
        } else {
            let l = parse_len(&mut c, true)?;
            if n == 3 {
                break; // z offset
            }
            if x.is_none() && n == 1 {
                x = Some(l);
            } else if y.is_none() {
                y = Some(l);
            } else if x.is_none() {
                x = Some(l);
            } else {
                return None;
            }
        }
    }
    if !c.at_end() || n == 0 {
        return None;
    }
    Some(Value::Pair(x.unwrap_or(Len::Pct(50.0)), y.unwrap_or(Len::Pct(50.0))))
}

fn parse_bg_size_one(t: &[Tok]) -> Option<BgSize> {
    let t = trim(t);
    match single_kw(t) {
        Some(Kw::Cover) => return Some(BgSize::Cover),
        Some(Kw::Contain) => return Some(BgSize::Contain),
        Some(Kw::Auto) => return Some(BgSize::Auto),
        _ => {}
    }
    let mut c = Cur::new(t);
    let mut dims: Vec<Option<Len>> = Vec::new();
    while !c.at_end() && dims.len() < 2 {
        if let Some(l) = parse_len(&mut c, false) {
            dims.push(Some(l));
        } else if c.ident().map_or(false, |i| i.eq_ignore_ascii_case("auto")) {
            dims.push(None);
        } else {
            return None;
        }
    }
    if dims.is_empty() || !c.at_end() {
        return None;
    }
    let w = dims[0].clone();
    let h = dims.get(1).cloned().unwrap_or(None);
    Some(BgSize::Explicit(w, h))
}

fn parse_repeat_one(t: &[Tok]) -> Option<(bool, bool)> {
    let kws: Vec<Kw> = t
        .iter()
        .filter(|x| !matches!(x, Tok::Whitespace))
        .map(|x| match x {
            Tok::Ident(i) => Kw::from_ident(i),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let one = |k: Kw| match k {
        Kw::Repeat | Kw::Space | Kw::Round => Some(true),
        Kw::NoRepeat => Some(false),
        _ => None,
    };
    match kws.as_slice() {
        [Kw::RepeatX] => Some((true, false)),
        [Kw::RepeatY] => Some((false, true)),
        [k] => one(*k).map(|v| (v, v)),
        [a, b] => Some((one(*a)?, one(*b)?)),
        _ => None,
    }
}

/// Parse the value of a longhand property.
pub fn parse_longhand(prop: PropId, toks: &[Tok], resolve: UrlResolver) -> Option<Value> {
    use PropId::*;
    match prop {
        Display => {
            // two-keyword syntax: keep the inner/outer keyword that matters to us
            let kws: Vec<Kw> = toks
                .iter()
                .filter_map(|t| match t {
                    Tok::Ident(i) => Some(Kw::from_ident(i)),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            let k = match kws.as_slice() {
                [k] => *k,
                [Kw::Inline, Kw::Flex] | [Kw::Flex, Kw::Inline] => Kw::InlineFlex,
                [Kw::Inline, Kw::Grid] | [Kw::Grid, Kw::Inline] => Kw::InlineGrid,
                [Kw::Block, k] | [k, Kw::Block] => *k,
                [Kw::Inline, Kw::FlowRoot] | [Kw::FlowRoot, Kw::Inline] => Kw::InlineBlock,
                _ => return None,
            };
            if let [Tok::Ident(i)] = toks {
                let l = i.to_ascii_lowercase();
                if l == "-webkit-box" || l == "-webkit-flex" || l == "-ms-flexbox" {
                    return Some(Value::Kw(Kw::Flex));
                }
                if l == "-webkit-inline-box" || l == "-webkit-inline-flex" {
                    return Some(Value::Kw(Kw::InlineFlex));
                }
            }
            matches!(
                k,
                Kw::None | Kw::Block | Kw::Inline | Kw::InlineBlock | Kw::Flex | Kw::InlineFlex | Kw::Grid
                    | Kw::InlineGrid | Kw::Table | Kw::InlineTable | Kw::TableRow | Kw::TableCell
                    | Kw::TableRowGroup | Kw::TableHeaderGroup | Kw::TableFooterGroup | Kw::TableColumn
                    | Kw::TableColumnGroup | Kw::TableCaption | Kw::ListItem | Kw::Contents | Kw::FlowRoot
            )
            .then_some(Value::Kw(k))
        }
        Position => {
            if let [Tok::Ident(i)] = toks {
                if i.eq_ignore_ascii_case("-webkit-sticky") {
                    return Some(Value::Kw(Kw::Sticky));
                }
            }
            kw_in(toks, &[Kw::Static, Kw::Relative, Kw::Absolute, Kw::Fixed, Kw::Sticky])
        }
        Float => kw_in(toks, &[Kw::None, Kw::Left, Kw::Right, Kw::InlineStart, Kw::InlineEnd]),
        Clear => kw_in(toks, &[Kw::None, Kw::Left, Kw::Right, Kw::Both, Kw::InlineStart, Kw::InlineEnd]),
        BoxSizing => kw_in(toks, &[Kw::ContentBox, Kw::BorderBox]),
        Visibility => kw_in(toks, &[Kw::Visible, Kw::Hidden, Kw::Collapse]),
        Opacity => {
            let mut c = Cur::new(toks);
            c.skip_ws();
            match c.peek()? {
                Tok::Percentage(p) => Some(Value::Num(p / 100.0)),
                _ => parse_number(&mut c).map(Value::Num),
            }
        }
        ZIndex => match toks {
            [Tok::Number(n)] => Some(Value::Int(*n as i32)),
            _ => kw_in(toks, &[Kw::Auto]),
        },
        OverflowX | OverflowY => kw_in(toks, &[Kw::Visible, Kw::Hidden, Kw::Scroll, Kw::Auto, Kw::Clip, Kw::Overlay]),
        Width | Height | MinWidth | MinHeight => parse_size(toks, SIZE_KWS),
        MaxWidth | MaxHeight => parse_size(toks, MAX_SIZE_KWS),
        MarginTop | MarginRight | MarginBottom | MarginLeft | Top | Right | Bottom | Left => {
            len_or_kw(toks, &[Kw::Auto])
        }
        PaddingTop | PaddingRight | PaddingBottom | PaddingLeft | TextIndent | OutlineOffset => {
            len_only(toks).map(Value::Len)
        }
        BorderTopWidth | BorderRightWidth | BorderBottomWidth | BorderLeftWidth | OutlineWidth => {
            let mut c = Cur::new(toks);
            let v = parse_border_width_tok(&mut c)?;
            c.at_end().then_some(v)
        }
        BorderTopStyle | BorderRightStyle | BorderBottomStyle | BorderLeftStyle => kw_in(toks, BORDER_STYLES),
        OutlineStyle => kw_in(toks, BORDER_STYLES).or_else(|| kw_in(toks, &[Kw::Auto])),
        BorderTopColor | BorderRightColor | BorderBottomColor | BorderLeftColor | Color | BackgroundColor
        | OutlineColor | TextDecorationColor | TextFillColor => color_only(toks),
        BackgroundClip => kw_in(toks, &[Kw::BorderBox, Kw::PaddingBox, Kw::ContentBox, Kw::Text]),
        BorderTopLeftRadius | BorderTopRightRadius | BorderBottomRightRadius | BorderBottomLeftRadius => {
            let mut c = Cur::new(toks);
            let l = parse_len(&mut c, false)?;
            Some(Value::Len(l))
        }
        BackgroundImage => {
            let mut imgs = Vec::new();
            for part in split_commas(toks) {
                let mut c = Cur::new(part);
                let img = parse_image(&mut c, resolve)?;
                if !c.at_end() {
                    return None;
                }
                imgs.push(img);
            }
            Some(Value::Images(imgs.into()))
        }
        BackgroundSize => {
            let v: Option<Vec<BgSize>> = split_commas(toks).into_iter().map(parse_bg_size_one).collect();
            Some(Value::BgSizes(v?.into()))
        }
        BackgroundPosition => {
            let mut v = Vec::new();
            for part in split_commas(toks) {
                let mut c = Cur::new(part);
                let p = parse_position(&mut c)?;
                if !c.at_end() {
                    return None;
                }
                v.push(p);
            }
            Some(Value::Positions(v.into()))
        }
        BackgroundRepeat => {
            let v: Option<Vec<(bool, bool)>> = split_commas(toks).into_iter().map(parse_repeat_one).collect();
            Some(Value::Repeats(v?.into()))
        }
        FontFamily => parse_families(toks),
        FontSize => parse_font_size(toks),
        FontWeight => parse_font_weight(toks),
        FontStyle => match toks.first()? {
            Tok::Ident(i) => match Kw::from_ident(i)? {
                k @ (Kw::Normal | Kw::Italic | Kw::Oblique) => Some(Value::Kw(k)),
                _ => None,
            },
            _ => None,
        },
        LineHeight => parse_line_height(toks),
        TextAlign => {
            if let [Tok::Ident(i)] = toks {
                let l = i.to_ascii_lowercase();
                if l == "-webkit-center" || l == "-moz-center" {
                    // centres block-level children too (what <center> and align=center do)
                    return Some(Value::Kw(Kw::WebkitCenter));
                }
                if let Some(rest) = l.strip_prefix("-webkit-").or_else(|| l.strip_prefix("-moz-")) {
                    return Kw::from_ident(rest).map(Value::Kw);
                }
            }
            kw_in(toks, &[Kw::Left, Kw::Right, Kw::Center, Kw::Justify, Kw::Start, Kw::End])
        }
        TextDecorationLine => parse_decoration_line(toks),
        TextTransform => kw_in(toks, &[Kw::None, Kw::Uppercase, Kw::Lowercase, Kw::Capitalize]),
        LetterSpacing | WordSpacing => len_or_kw(toks, &[Kw::Normal]),
        WhiteSpace => kw_in(toks, &[Kw::Normal, Kw::Nowrap, Kw::Pre, Kw::PreWrap, Kw::PreLine, Kw::BreakSpaces]),
        VerticalAlign => len_or_kw(
            toks,
            &[Kw::Baseline, Kw::Sub, Kw::Super, Kw::Top, Kw::Middle, Kw::Bottom, Kw::TextTop, Kw::TextBottom],
        ),
        WordBreak => kw_in(toks, &[Kw::Normal, Kw::BreakAll, Kw::KeepAll, Kw::BreakWord]),
        OverflowWrap => kw_in(toks, &[Kw::Normal, Kw::BreakWord, Kw::Anywhere]),
        TextOverflow => kw_in(toks, &[Kw::Clip, Kw::Ellipsis]),
        ListStyleType => {
            if let [Tok::Str(_)] = toks {
                return Some(Value::Kw(Kw::Disc));
            }
            kw_in(
                toks,
                &[
                    Kw::None, Kw::Disc, Kw::Circle, Kw::Square, Kw::Decimal, Kw::DecimalLeadingZero,
                    Kw::LowerAlpha, Kw::UpperAlpha, Kw::LowerLatin, Kw::UpperLatin, Kw::LowerRoman,
                    Kw::UpperRoman,
                ],
            )
        }
        Cursor => {
            // last keyword of a possibly url()-prefixed list
            let last = split_commas(toks).pop()?;
            match single_kw(trim(last)) {
                Some(k @ (Kw::Pointer | Kw::Text | Kw::Move | Kw::NotAllowed | Kw::Grab | Kw::Grabbing
                | Kw::Crosshair | Kw::Help | Kw::Wait | Kw::Progress | Kw::ZoomIn | Kw::ZoomOut
                | Kw::ColResize | Kw::RowResize | Kw::EwResize | Kw::NsResize)) => Some(Value::Kw(k)),
                Some(_) => Some(Value::Kw(Kw::Default)),
                None => None,
            }
        }
        PointerEvents => match single_kw(toks)? {
            Kw::None => Some(Value::Kw(Kw::None)),
            _ => Some(Value::Kw(Kw::Auto)),
        },
        FlexDirection => kw_in(toks, &[Kw::Row, Kw::RowReverse, Kw::Column, Kw::ColumnReverse]),
        FlexWrap => kw_in(toks, &[Kw::Nowrap, Kw::Wrap, Kw::WrapReverse]),
        JustifyContent | AlignItems | AlignSelf | AlignContent | JustifyItems | JustifySelf => parse_align(toks),
        FlexGrow | FlexShrink => {
            let mut c = Cur::new(toks);
            let n = parse_number(&mut c)?;
            (n >= 0.0 && c.at_end()).then_some(Value::Num(n))
        }
        FlexBasis => len_or_kw(toks, &[Kw::Auto, Kw::Content, Kw::MinContent, Kw::MaxContent, Kw::FitContent]),
        Order => match toks {
            [Tok::Number(n)] => Some(Value::Int(*n as i32)),
            _ => None,
        },
        RowGap | ColumnGap => len_or_kw(toks, &[Kw::Normal]),
        GridTemplateColumns | GridTemplateRows | GridAutoRows | GridAutoColumns => {
            if kw_in(toks, &[Kw::None, Kw::Auto]).is_some() {
                return Some(Value::Kw(Kw::None));
            }
            parse_track_list(toks).map(|t| Value::Tracks(t.into()))
        }
        GridColumnStart | GridColumnEnd | GridRowStart | GridRowEnd => {
            let mut c = Cur::new(toks);
            parse_grid_line(&mut c).map(Value::GridLine)
        }
        GridAutoFlow => {
            let col = toks.iter().any(|t| t.is_ident("column"));
            Some(Value::Kw(if col { Kw::Column } else { Kw::Row }))
        }
        GridTemplateAreas => {
            if kw_in(toks, &[Kw::None]).is_some() {
                return Some(Value::Kw(Kw::None));
            }
            let mut rows: Vec<&str> = Vec::new();
            for t in toks {
                match t {
                    Tok::Str(s) => rows.push(s),
                    Tok::Whitespace => {}
                    _ => return None,
                }
            }
            parse_grid_areas(&rows).map(|a| Value::Areas(Rc::new(a)))
        }
        GridAreaName => match toks {
            [Tok::Ident(i)] if !i.eq_ignore_ascii_case("auto") => Some(Value::Name(Atom::new(i))),
            _ => Some(Value::Kw(Kw::None)),
        },
        MaskImage => {
            // Masks are not rendered yet; we only need to know whether one is set.
            if kw_in(toks, &[Kw::None]).is_some() {
                Some(Value::Kw(Kw::None))
            } else if toks.iter().any(|t| matches!(t, Tok::Url(_) | Tok::Function(_))) {
                Some(Value::Int(1))
            } else {
                None
            }
        }
        BoxShadow => {
            if kw_in(toks, &[Kw::None]).is_some() {
                return Some(Value::Kw(Kw::None));
            }
            parse_shadows(toks).map(|s| Value::Shadows(s.into()))
        }
        BorderCollapse => kw_in(toks, &[Kw::Collapse, Kw::Separate]),
        BorderSpacing => {
            let mut c = Cur::new(toks);
            let a = parse_len(&mut c, false)?;
            let b = parse_len(&mut c, false).unwrap_or_else(|| a.clone());
            Some(Value::Pair(a, b))
        }
        TableLayout => kw_in(toks, &[Kw::Auto, Kw::Fixed]),
        Content => parse_content(toks),
        ObjectFit => kw_in(toks, &[Kw::Fill, Kw::Contain, Kw::Cover, Kw::None, Kw::ScaleDown]),
        AspectRatio => {
            let mut c = Cur::new(toks);
            if c.clone().ident().map_or(false, |i| i.eq_ignore_ascii_case("auto")) {
                c.ident();
                if c.at_end() {
                    return Some(Value::Kw(Kw::Auto));
                }
            }
            let a = parse_number(&mut c)?;
            let b = if c.delim('/') { parse_number(&mut c)? } else { 1.0 };
            (a > 0.0 && b > 0.0).then_some(Value::Num(a / b))
        }
        Transform => parse_transform(toks),
        Translate => parse_translate_prop(toks),
        Rotate => parse_rotate_prop(toks),
        Scale => parse_scale_prop(toks),
        TransformOrigin => parse_transform_origin(toks),
        TransitionProperty => comma_list(toks, |c| c.ident().map(trans_prop)).map(|v| Value::TransProps(v.into())),
        TransitionDuration | AnimationDuration => {
            comma_list(toks, |c| parse_time(c).filter(|t| *t >= 0.0)).map(|v| Value::Times(v.into()))
        }
        TransitionDelay | AnimationDelay => comma_list(toks, parse_time).map(|v| Value::Times(v.into())),
        TransitionTimingFunction | AnimationTimingFunction => comma_list(toks, Easing::parse).map(|v| Value::Easings(v.into())),
        AnimationName => comma_list(toks, |c| {
            c.skip_ws();
            match c.next()? {
                Tok::Ident(i) if i.eq_ignore_ascii_case("none") => Some(Atom::NONE),
                Tok::Ident(i) | Tok::Str(i) => Some(Atom::new(i)),
                _ => None,
            }
        })
        .map(|v| Value::Names(v.into())),
        AnimationIterationCount => comma_list(toks, |c| {
            c.skip_ws();
            match c.peek() {
                Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("infinite") => {
                    c.next();
                    Some(f32::INFINITY)
                }
                _ => parse_number(c).filter(|n| *n >= 0.0),
            }
        })
        .map(|v| Value::Nums(v.into())),
        AnimationDirection => ident_code_list(toks, direction_code),
        AnimationFillMode => ident_code_list(toks, fill_code),
        AnimationPlayState => ident_code_list(toks, play_code),
        Custom => None,
        _ => None,
    }
}

fn push(out: &mut Vec<Declaration>, prop: PropId, value: Value) {
    out.push(Declaration { prop, value, important: false });
}

/// Split whitespace-separated top-level components (functions stay intact).
fn components(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start: Option<usize> = None;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Whitespace if depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&toks[s..i]);
                }
                continue;
            }
            Tok::Function(_) | Tok::LParen | Tok::LBracket => depth += 1,
            Tok::RParen | Tok::RBracket => depth -= 1,
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
        // a '/' is its own component
        if depth == 0 && matches!(t, Tok::Delim('/')) {
            let s = start.take().unwrap();
            if s < i {
                out.push(&toks[s..i]);
            }
            out.push(&toks[i..i + 1]);
        }
    }
    if let Some(s) = start {
        out.push(&toks[s..]);
    }
    out
}

fn expand_four(
    toks: &[Tok],
    props: &[PropId],
    resolve: UrlResolver,
    out: &mut Vec<Declaration>,
) -> Option<()> {
    let comps = components(toks);
    if comps.is_empty() || comps.len() > 4 {
        return None;
    }
    let vals: Option<Vec<Value>> = comps.iter().map(|c| parse_longhand(props[0], c, resolve)).collect();
    let vals = vals?;
    let pick = |i: usize| -> Value {
        match (vals.len(), i) {
            (1, _) => vals[0].clone(),
            (2, 0) | (2, 2) => vals[0].clone(),
            (2, _) => vals[1].clone(),
            (3, 0) => vals[0].clone(),
            (3, 1) | (3, 3) => vals[1].clone(),
            (3, _) => vals[2].clone(),
            (_, i) => vals[i].clone(),
        }
    };
    for (i, &p) in props.iter().enumerate() {
        push(out, p, pick(i));
    }
    Some(())
}

fn expand_two(toks: &[Tok], props: &[PropId], resolve: UrlResolver, out: &mut Vec<Declaration>) -> Option<()> {
    let comps: Vec<&[Tok]> = components(toks).into_iter().filter(|c| !matches!(c, [Tok::Delim('/')])).collect();
    if comps.is_empty() || comps.len() > 2 {
        return None;
    }
    let a = parse_longhand(props[0], comps[0], resolve)?;
    let b = match comps.get(1) {
        Some(c) => parse_longhand(props[1], c, resolve)?,
        None => a.clone(),
    };
    push(out, props[0], a);
    push(out, props[1], b);
    Some(())
}

/// `<width> || <style> || <color>` in any order.
fn parse_border_parts(toks: &[Tok]) -> Option<(Value, Value, Value)> {
    let mut width = Value::Len(Len::Px(3.0));
    let mut style = Value::Kw(Kw::None);
    let mut color = Value::Color(CssColor::Current);
    let mut c = Cur::new(toks);
    let mut n = 0;
    while !c.at_end() {
        n += 1;
        if n > 3 {
            return None;
        }
        let save = c.i;
        if let Some(i) = c.ident() {
            if let Some(k) = Kw::from_ident(i) {
                if BORDER_STYLES.contains(&k) {
                    style = Value::Kw(k);
                    continue;
                }
            }
            c.i = save;
        }
        if let Some(w) = parse_border_width_tok(&mut c) {
            width = w;
            continue;
        }
        if let Some(col) = parse_color(&mut c) {
            color = Value::Color(col);
            continue;
        }
        return None;
    }
    Some((width, style, color))
}

fn expand_background(toks: &[Tok], resolve: UrlResolver, out: &mut Vec<Declaration>) -> Option<()> {
    let layers = split_commas(toks);
    let mut color = CssColor::Rgba(Color::TRANSPARENT);
    let mut images = Vec::new();
    let mut sizes = Vec::new();
    let mut positions = Vec::new();
    let mut repeats = Vec::new();
    let mut clip_text = false;
    let n = layers.len();
    for (li, layer) in layers.into_iter().enumerate() {
        let mut c = Cur::new(layer);
        let mut image = Image::None;
        let mut size = BgSize::Auto;
        let mut pos = (Len::Pct(0.0), Len::Pct(0.0));
        let mut repeat = (true, true);
        let mut guard = 0;
        while !c.at_end() {
            guard += 1;
            if guard > 16 {
                return None;
            }
            if let Some(img) = parse_image(&mut c, resolve) {
                image = img;
                continue;
            }
            if let Some(col) = parse_color(&mut c) {
                if li != n - 1 {
                    return None;
                }
                color = col;
                continue;
            }
            if c.delim('/') {
                // background-size after position
                let start = c.i;
                let mut end = c.i;
                let mut cc = c.clone();
                let mut count = 0;
                while count < 2 {
                    let s = cc.i;
                    if parse_len(&mut cc, false).is_some() {
                        end = cc.i;
                    } else if let Some(i) = cc.ident() {
                        if matches!(Kw::from_ident(i), Some(Kw::Auto | Kw::Cover | Kw::Contain)) {
                            end = cc.i;
                        } else {
                            cc.i = s;
                            break;
                        }
                    } else {
                        break;
                    }
                    count += 1;
                }
                size = parse_bg_size_one(&c.t[start..end])?;
                c.i = end;
                continue;
            }
            let save = c.i;
            if let Some(i) = c.ident() {
                match Kw::from_ident(i) {
                    Some(Kw::Repeat | Kw::Space | Kw::Round) => {
                        repeat = (true, true);
                        continue;
                    }
                    Some(Kw::NoRepeat) => {
                        repeat = (false, false);
                        continue;
                    }
                    Some(Kw::RepeatX) => {
                        repeat = (true, false);
                        continue;
                    }
                    Some(Kw::RepeatY) => {
                        repeat = (false, true);
                        continue;
                    }
                    Some(k @ (Kw::Scroll | Kw::Fixed | Kw::Local | Kw::BorderBox | Kw::PaddingBox | Kw::ContentBox | Kw::Text)) => {
                        clip_text |= k == Kw::Text;
                        continue;
                    }
                    _ => c.i = save,
                }
            }
            if let Some(p) = parse_position(&mut c) {
                pos = p;
                continue;
            }
            return None;
        }
        images.push(image);
        sizes.push(size);
        positions.push(pos);
        repeats.push(repeat);
    }
    push(out, PropId::BackgroundColor, Value::Color(color));
    push(out, PropId::BackgroundImage, Value::Images(images.into()));
    push(out, PropId::BackgroundSize, Value::BgSizes(sizes.into()));
    push(out, PropId::BackgroundPosition, Value::Positions(positions.into()));
    push(out, PropId::BackgroundRepeat, Value::Repeats(repeats.into()));
    push(out, PropId::BackgroundClip, Value::Kw(if clip_text { Kw::Text } else { Kw::BorderBox }));
    Some(())
}

fn expand_font(toks: &[Tok], out: &mut Vec<Declaration>) -> Option<()> {
    let comps = components(toks);
    let mut style = Value::Kw(Kw::Normal);
    let mut weight = Value::Num(400.0);
    let mut i = 0;
    // leading style / variant / weight / stretch keywords
    while i < comps.len() {
        let c = comps[i];
        if let Some(k) = single_kw(c) {
            match k {
                Kw::Normal => {}
                Kw::Italic | Kw::Oblique => style = Value::Kw(k),
                Kw::Bold | Kw::Bolder | Kw::Lighter => weight = parse_font_weight(c)?,
                _ => {
                    if parse_font_size(c).is_some() {
                        break;
                    }
                    // small-caps, condensed, ...: ignored
                    if !matches!(c, [Tok::Ident(_)]) {
                        return None;
                    }
                }
            }
            i += 1;
            continue;
        }
        if let [Tok::Number(_)] = c {
            weight = parse_font_weight(c)?;
            i += 1;
            continue;
        }
        if let [Tok::Ident(_)] = c {
            if parse_font_size(c).is_none() {
                i += 1; // unknown keyword such as small-caps
                continue;
            }
        }
        break;
    }
    let size = parse_font_size(comps.get(i)?)?;
    i += 1;
    let mut line_height = Value::Kw(Kw::Normal);
    if matches!(comps.get(i), Some([Tok::Delim('/')])) {
        line_height = parse_line_height(comps.get(i + 1)?)?;
        i += 2;
    }
    if i >= comps.len() {
        return None;
    }
    // family: everything after, re-joined from the original token slice
    let first = comps[i].as_ptr() as usize;
    let base = toks.as_ptr() as usize;
    let idx = (first - base) / std::mem::size_of::<Tok>();
    let family = parse_families(&toks[idx..])?;
    push(out, PropId::FontStyle, style);
    push(out, PropId::FontWeight, weight);
    push(out, PropId::FontSize, size);
    push(out, PropId::LineHeight, line_height);
    push(out, PropId::FontFamily, family);
    Some(())
}

fn expand_flex(toks: &[Tok], out: &mut Vec<Declaration>) -> Option<()> {
    let (grow, shrink, basis): (f32, f32, Value);
    match single_kw(toks) {
        Some(Kw::None) => {
            grow = 0.0;
            shrink = 0.0;
            basis = Value::Kw(Kw::Auto);
        }
        Some(Kw::Auto) => {
            grow = 1.0;
            shrink = 1.0;
            basis = Value::Kw(Kw::Auto);
        }
        _ => {
            let comps = components(toks);
            let mut nums: Vec<f32> = Vec::new();
            let mut b: Option<Value> = None;
            for c in &comps {
                match c {
                    [Tok::Number(n)] if nums.len() < 2 && b.is_none() => nums.push(*n),
                    // a third unitless 0 is a basis
                    [Tok::Number(n)] if *n == 0.0 => b = Some(Value::Len(Len::Px(0.0))),
                    _ => {
                        b = Some(parse_longhand(PropId::FlexBasis, c, &|_| None)?);
                    }
                }
            }
            if comps.is_empty() || comps.len() > 3 {
                return None;
            }
            grow = nums.first().copied().unwrap_or(1.0);
            shrink = nums.get(1).copied().unwrap_or(1.0);
            basis = match b {
                Some(v) => v,
                None => Value::Len(Len::Pct(0.0)),
            };
            if nums.is_empty() && comps.len() != 1 {
                return None;
            }
        }
    }
    push(out, PropId::FlexGrow, Value::Num(grow));
    push(out, PropId::FlexShrink, Value::Num(shrink));
    push(out, PropId::FlexBasis, basis);
    Some(())
}

fn expand_shorthand(prop: PropId, toks: &[Tok], resolve: UrlResolver, out: &mut Vec<Declaration>) -> Option<()> {
    use PropId::*;
    match prop {
        Transition => expand_transition(toks, out),
        Animation => expand_animation(toks, out),
        Margin | Padding | Inset | BorderWidth | BorderStyle | BorderColor => {
            expand_four(toks, longhands(prop), resolve, out)
        }
        MarginInline | MarginBlock | PaddingInline | PaddingBlock | InsetInline | InsetBlock | Gap | Overflow
        | PlaceItems | PlaceContent | PlaceSelf => expand_two(toks, longhands(prop), resolve, out),
        BorderRadius => {
            // horizontal radii only (elliptical corners are approximated as circular)
            let h: Vec<Tok> = toks.iter().take_while(|t| !matches!(t, Tok::Delim('/'))).cloned().collect();
            expand_four(&h, longhands(prop), resolve, out)
        }
        Border | BorderTop | BorderRight | BorderBottom | BorderLeft | BorderInline | BorderBlock => {
            let (w, s, c) = parse_border_parts(toks)?;
            for &l in longhands(prop) {
                let name = l.name();
                let v = if name.ends_with("width") {
                    w.clone()
                } else if name.ends_with("style") {
                    s.clone()
                } else {
                    c.clone()
                };
                push(out, l, v);
            }
            Some(())
        }
        Outline => {
            if let Some(Kw::None) = single_kw(toks) {
                push(out, OutlineStyle, Value::Kw(Kw::None));
                push(out, OutlineWidth, Value::Len(Len::Px(0.0)));
                return Some(());
            }
            // `auto` style is treated as solid
            let cleaned: Vec<Tok> = toks
                .iter()
                .map(|t| if t.is_ident("auto") { Tok::Ident("solid".into()) } else { t.clone() })
                .collect();
            let (w, s, c) = parse_border_parts(&cleaned)?;
            push(out, OutlineWidth, w);
            push(out, OutlineStyle, s);
            push(out, OutlineColor, c);
            Some(())
        }
        Background => expand_background(toks, resolve, out),
        Font => expand_font(toks, out),
        Flex => expand_flex(toks, out),
        FlexFlow => {
            let mut dir = Value::Kw(Kw::Row);
            let mut wrap = Value::Kw(Kw::Nowrap);
            for c in components(toks) {
                if let Some(v) = parse_longhand(FlexDirection, c, resolve) {
                    dir = v;
                } else if let Some(v) = parse_longhand(FlexWrap, c, resolve) {
                    wrap = v;
                } else {
                    return None;
                }
            }
            push(out, FlexDirection, dir);
            push(out, FlexWrap, wrap);
            Some(())
        }
        ListStyle => {
            let mut ty: Option<Value> = None;
            for c in components(toks) {
                if let Some(Kw::None) = single_kw(c) {
                    ty.get_or_insert(Value::Kw(Kw::None));
                } else if let Some(v) = parse_longhand(ListStyleType, c, resolve) {
                    ty = Some(v);
                }
            }
            push(out, ListStyleType, ty.unwrap_or(Value::Kw(Kw::Disc)));
            Some(())
        }
        TextDecoration => {
            let mut bits = 0;
            let mut color: Option<CssColor> = None;
            let mut c = Cur::new(toks);
            while !c.at_end() {
                if let Some(col) = parse_color(&mut c) {
                    color = Some(col);
                    continue;
                }
                if parse_len(&mut c, false).is_some() {
                    continue; // thickness
                }
                match Kw::from_ident(c.ident()?) {
                    Some(Kw::Underline) => bits |= 1,
                    Some(Kw::Overline) => bits |= 2,
                    Some(Kw::LineThrough) => bits |= 4,
                    Some(Kw::None | Kw::Solid | Kw::Double | Kw::Dotted | Kw::Dashed | Kw::Wavy | Kw::Auto) => {}
                    _ => return None,
                }
            }
            push(out, TextDecorationLine, Value::Int(bits));
            if let Some(col) = color {
                push(out, TextDecorationColor, Value::Color(col));
            }
            Some(())
        }
        GridColumn | GridRow => {
            let parts: Vec<&[Tok]> = toks.split(|t| matches!(t, Tok::Delim('/'))).collect();
            let ls = longhands(prop);
            let start = parse_grid_line(&mut Cur::new(parts[0]))?;
            let end = match parts.get(1) {
                Some(p) => parse_grid_line(&mut Cur::new(p))?,
                None => GridLine::Auto,
            };
            push(out, ls[0], Value::GridLine(start));
            push(out, ls[1], Value::GridLine(end));
            Some(())
        }
        GridArea => {
            let parts: Vec<&[Tok]> = toks.split(|t| matches!(t, Tok::Delim('/'))).collect();
            let ls = longhands(prop);
            if parts.len() == 1 {
                // a single identifier names an area from the container's grid-template-areas
                if let [Tok::Ident(i)] = trim(parts[0]) {
                    if !i.eq_ignore_ascii_case("auto") && !i.eq_ignore_ascii_case("span") {
                        for &l in &ls[..4] {
                            push(out, l, Value::GridLine(GridLine::Auto));
                        }
                        push(out, GridAreaName, Value::Name(Atom::new(i)));
                        return Some(());
                    }
                }
            }
            for (i, &l) in ls[..4].iter().enumerate() {
                let v = match parts.get(i) {
                    Some(p) => parse_grid_line(&mut Cur::new(p))?,
                    None => GridLine::Auto,
                };
                push(out, l, Value::GridLine(v));
            }
            push(out, GridAreaName, Value::Kw(Kw::None));
            Some(())
        }
        GridTemplate => {
            if let Some(Kw::None) = single_kw(toks) {
                push(out, GridTemplateRows, Value::Kw(Kw::None));
                push(out, GridTemplateColumns, Value::Kw(Kw::None));
                push(out, GridTemplateAreas, Value::Kw(Kw::None));
                return Some(());
            }
            let parts: Vec<&[Tok]> = toks.split(|t| matches!(t, Tok::Delim('/'))).collect();
            if parts.len() > 2 {
                return None;
            }
            if parts[0].iter().any(|t| matches!(t, Tok::Str(_))) {
                // `"a b" 40px "c d" 1fr / <columns>`: area rows, each optionally followed by its size
                let mut area_rows: Vec<&str> = Vec::new();
                let mut row_tracks: Vec<TrackItem> = Vec::new();
                let auto = || TrackItem::One(Track { min: TrackBreadth::Auto, max: TrackBreadth::Auto });
                let mut awaiting_size = false;
                for c in components(parts[0]) {
                    match c {
                        [Tok::Str(s)] => {
                            if awaiting_size {
                                row_tracks.push(auto());
                            }
                            area_rows.push(s);
                            awaiting_size = true;
                        }
                        [Tok::LBracket, ..] => {}
                        other => {
                            if !awaiting_size {
                                return None;
                            }
                            row_tracks.extend(parse_track_list(other)?);
                            awaiting_size = false;
                        }
                    }
                }
                if awaiting_size {
                    row_tracks.push(auto());
                }
                let areas = parse_grid_areas(&area_rows)?;
                let cols = match parts.get(1) {
                    Some(p) => parse_longhand(GridTemplateColumns, trim(p), resolve)?,
                    None => Value::Kw(Kw::None),
                };
                push(out, GridTemplateAreas, Value::Areas(Rc::new(areas)));
                push(out, GridTemplateRows, Value::Tracks(row_tracks.into()));
                push(out, GridTemplateColumns, cols);
                return Some(());
            }
            if parts.len() != 2 {
                return None;
            }
            let rows = parse_longhand(GridTemplateRows, trim(parts[0]), resolve)?;
            let cols = parse_longhand(GridTemplateColumns, trim(parts[1]), resolve)?;
            push(out, GridTemplateRows, rows);
            push(out, GridTemplateColumns, cols);
            push(out, GridTemplateAreas, Value::Kw(Kw::None));
            Some(())
        }
        _ => None,
    }
}

/// Parse the contents of a `style` attribute or a declaration block body (already tokenized).
pub fn parse_declaration_list(src: &str, resolve: UrlResolver) -> Vec<Declaration> {
    let tokens = super::tokenizer::tokenize(src);
    let mut out = Vec::new();
    parse_declarations(&tokens.toks, &tokens.spans, src, 0, tokens.toks.len(), resolve, &mut out);
    out
}

/// Parse declarations in `toks[start..end]` (the inside of a `{}` block).
pub fn parse_declarations(
    toks: &[Tok],
    spans: &[(u32, u32)],
    src: &str,
    start: usize,
    end: usize,
    resolve: UrlResolver,
    out: &mut Vec<Declaration>,
) {
    let mut i = start;
    while i < end {
        // skip whitespace and stray semicolons
        while i < end && matches!(toks[i], Tok::Whitespace | Tok::Semicolon) {
            i += 1;
        }
        if i >= end {
            break;
        }
        // find the end of this declaration (top-level ';')
        let decl_start = i;
        let mut depth = 0;
        let mut j = i;
        while j < end {
            match &toks[j] {
                Tok::Function(_) | Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth -= 1,
                Tok::Semicolon if depth <= 0 => break,
                _ => {}
            }
            j += 1;
        }
        let decl_end = j;
        i = j + 1;

        let Tok::Ident(name) = &toks[decl_start] else { continue };
        let mut k = decl_start + 1;
        while k < decl_end && matches!(toks[k], Tok::Whitespace) {
            k += 1;
        }
        if k >= decl_end || !matches!(toks[k], Tok::Colon) {
            continue;
        }
        k += 1;
        let mut value_end = decl_end;
        // strip trailing whitespace and `!important`
        let mut important = false;
        let mut e = value_end;
        while e > k && matches!(toks[e - 1], Tok::Whitespace) {
            e -= 1;
        }
        if e >= k + 2 && toks[e - 1].is_ident("important") {
            let mut b = e - 1;
            while b > k && matches!(toks[b - 1], Tok::Whitespace) {
                b -= 1;
            }
            if b > k && matches!(toks[b - 1], Tok::Delim('!')) {
                important = true;
                value_end = b - 1;
            }
        }
        if value_end <= k {
            if !name.starts_with("--") {
                continue;
            }
        }
        let raw = if value_end > k {
            &src[spans[k].0 as usize..spans[value_end - 1].1 as usize]
        } else {
            ""
        };
        parse_declaration(name, &toks[k..value_end.max(k)], raw, important, resolve, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decls(s: &str) -> Vec<Declaration> {
        parse_declaration_list(s, &|u| Some(u.to_string()))
    }

    #[test]
    fn shorthand_margin() {
        let d = decls("margin: 0 auto; padding: 1px 2px 3px");
        assert_eq!(d.len(), 8);
        assert_eq!(d[0].value, Value::Len(Len::Px(0.0)));
        assert_eq!(d[1].value, Value::Kw(Kw::Auto));
        assert_eq!(d[3].value, Value::Kw(Kw::Auto));
        assert_eq!(d[7].value, Value::Len(Len::Px(2.0)));
    }

    #[test]
    fn border_and_important() {
        let d = decls("border: 1px solid rgba(55, 65, 81, 0.5) !important; color: red");
        assert_eq!(d.len(), 13);
        assert!(d[0].important);
        assert!(!d[12].important);
        assert_eq!(d[12].value, Value::Color(CssColor::Rgba(Color::rgb(255, 0, 0))));
    }

    #[test]
    fn var_is_pending() {
        let d = decls("--x: 4px; padding: var(--x) 2px; background: linear-gradient(135deg, #2563eb, #1e40af)");
        assert!(matches!(d[0].value, Value::Custom(..)));
        assert!(matches!(d[1].value, Value::Pending(_)));
        assert_eq!(d[1].prop, PropId::Padding);
        assert!(d.iter().any(|x| x.prop == PropId::BackgroundImage));
    }

    #[test]
    fn flex_and_font() {
        let d = decls("flex: 1; font: italic 600 14px/1.4 'Geist', sans-serif");
        assert_eq!(d[0].value, Value::Num(1.0));
        assert_eq!(d[2].value, Value::Len(Len::Pct(0.0)));
        assert_eq!(d[3].value, Value::Kw(Kw::Italic));
        assert_eq!(d[4].value, Value::Num(600.0));
        assert_eq!(d[5].value, Value::Len(Len::Px(14.0)));
        assert_eq!(d[6].value, Value::Num(1.4));
    }
}
