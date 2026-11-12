//! Computed style: the fully resolved set of property values for one box.

use crate::atom::Atom;
use crate::css::values::{BgSize, Color, ContentItem, GridAreas, GridLine, Image, Len, Size, TrackItem};
use std::rc::Rc;

macro_rules! simple_enum {
    ($name:ident { $($v:ident),* $(,)? }) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        #[repr(u8)]
        pub enum $name { $($v),* }
    };
}

simple_enum!(Display {
    None, Block, Inline, InlineBlock, Flex, InlineFlex, Grid, InlineGrid, Table, InlineTable, TableRow,
    TableCell, TableRowGroup, TableHeaderGroup, TableFooterGroup, TableColumn, TableColumnGroup,
    TableCaption, ListItem, Contents,
});
simple_enum!(Position { Static, Relative, Absolute, Fixed, Sticky });
simple_enum!(Float { None, Left, Right });
simple_enum!(Clear { None, Left, Right, Both });
simple_enum!(Overflow { Visible, Hidden, Scroll, Auto, Clip });
simple_enum!(BorderStyle { None, Hidden, Solid, Dashed, Dotted, Double, Groove, Ridge, Inset, Outset });
simple_enum!(TextAlign { Left, Right, Center, Justify, BlockCenter });
simple_enum!(WhiteSpace { Normal, Nowrap, Pre, PreWrap, PreLine });
simple_enum!(TextTransform { None, Uppercase, Lowercase, Capitalize });
simple_enum!(ListStyle {
    None, Disc, Circle, Square, Decimal, DecimalLeadingZero, LowerAlpha, UpperAlpha, LowerRoman, UpperRoman,
});
simple_enum!(Cursor { Auto, Default, Pointer, Text, Move, NotAllowed, Grab, Crosshair, Help, Wait, ResizeH, ResizeV });
simple_enum!(FlexDirection { Row, RowReverse, Column, ColumnReverse });
simple_enum!(FlexWrap { Nowrap, Wrap, WrapReverse });
simple_enum!(Align {
    Auto, Normal, Stretch, Start, End, Center, Baseline, SpaceBetween, SpaceAround, SpaceEvenly,
});
simple_enum!(ObjectFit { Fill, Contain, Cover, None, ScaleDown });
simple_enum!(BoxSizing { ContentBox, BorderBox });
simple_enum!(Visibility { Visible, Hidden, Collapse });
simple_enum!(OverflowWrap { Normal, BreakWord, Anywhere });

impl Display {
    /// Participates in inline layout as a whole or as an inline box.
    pub fn is_inline_level(self) -> bool {
        matches!(self, Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable)
    }
    pub fn is_table_internal(self) -> bool {
        matches!(
            self,
            Display::TableRow | Display::TableCell | Display::TableRowGroup | Display::TableHeaderGroup
                | Display::TableFooterGroup | Display::TableColumn | Display::TableColumnGroup | Display::TableCaption
        )
    }
    pub fn is_flex(self) -> bool {
        matches!(self, Display::Flex | Display::InlineFlex)
    }
    pub fn is_grid(self) -> bool {
        matches!(self, Display::Grid | Display::InlineGrid)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VAlign {
    Baseline,
    Sub,
    Super,
    Top,
    Middle,
    Bottom,
    TextTop,
    TextBottom,
    Length(f32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Px(f32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct BgLayer {
    pub image: Image,
    pub size: BgSize,
    pub pos: (Len, Len),
    pub repeat: (bool, bool),
}

impl Default for BgLayer {
    fn default() -> Self {
        BgLayer { image: Image::None, size: BgSize::Auto, pos: (Len::Pct(0.0), Len::Pct(0.0)), repeat: (true, true) }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// Custom properties visible to an element: a persistent chain so children share their parent's
/// map instead of copying it.
#[derive(Debug)]
pub struct VarMap {
    pub parent: Option<Rc<VarMap>>,
    pub own: Vec<(Atom, Rc<str>)>,
}

impl VarMap {
    pub fn get(&self, name: Atom) -> Option<&Rc<str>> {
        let mut m = Some(self);
        while let Some(cur) = m {
            // later declarations win, so scan from the end
            if let Some((_, v)) = cur.own.iter().rev().find(|(n, _)| *n == name) {
                return Some(v);
            }
            m = cur.parent.as_deref();
        }
        None
    }
}

pub const DECO_UNDERLINE: u8 = 1;
pub const DECO_OVERLINE: u8 = 2;
pub const DECO_LINE_THROUGH: u8 = 4;

#[derive(Clone, Debug)]
pub struct ComputedStyle {
    // ── inherited ──
    pub color: Color,
    pub font_size: f32,
    pub font_weight: u16,
    pub italic: bool,
    pub font_family: Rc<[Atom]>,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub white_space: WhiteSpace,
    pub visibility: Visibility,
    pub cursor: Cursor,
    pub list_style: ListStyle,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_transform: TextTransform,
    pub text_indent: f32,
    pub border_collapse: bool,
    pub border_spacing: (f32, f32),
    pub overflow_wrap: OverflowWrap,
    pub break_all: bool,
    pub pointer_events: bool,
    /// Decoration lines in effect for text in this box (own lines plus propagated ones).
    pub text_decoration: u8,
    pub vars: Option<Rc<VarMap>>,

    // ── not inherited ──
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub box_sizing: BoxSizing,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    pub width: Size,
    pub height: Size,
    pub min_width: Size,
    pub min_height: Size,
    pub max_width: Size,
    pub max_height: Size,
    /// top, right, bottom, left
    pub margin: [Size; 4],
    pub padding: [Size; 4],
    pub inset: [Size; 4],
    pub border_width: [f32; 4],
    pub border_style: [BorderStyle; 4],
    pub border_color: [Color; 4],
    /// top-left, top-right, bottom-right, bottom-left
    pub border_radius: [Size; 4],
    pub background_color: Color,
    pub background: Option<Rc<Vec<BgLayer>>>,
    /// `background-clip: text`: the background shows only through this element's text.
    pub bg_clip_text: bool,
    /// Inherited: the background of the nearest ancestor-or-self with `background-clip: text`.
    pub text_bg: Option<Rc<Vec<BgLayer>>>,
    /// `-webkit-text-fill-color` (inherited); `None` paints text in `color`.
    pub text_fill: Option<Color>,
    pub box_shadow: Option<Rc<[Shadow]>>,
    pub opacity: f32,
    pub z_index: Option<i32>,
    pub vertical_align: VAlign,
    pub text_decoration_own: u8,
    pub text_overflow_ellipsis: bool,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: Align,
    pub align_items: Align,
    pub align_self: Align,
    pub align_content: Align,
    pub justify_items: Align,
    pub justify_self: Align,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Size,
    pub order: i32,
    pub row_gap: Size,
    pub column_gap: Size,
    pub grid_template_columns: Option<Rc<[TrackItem]>>,
    pub grid_template_rows: Option<Rc<[TrackItem]>>,
    pub grid_auto_rows: Option<Rc<[TrackItem]>>,
    pub grid_auto_flow_column: bool,
    pub grid_column: (GridLine, GridLine),
    pub grid_row: (GridLine, GridLine),
    pub grid_template_areas: Option<Rc<GridAreas>>,
    /// Named area this item is placed in (`Atom::NONE` if none).
    pub grid_area: Atom,
    /// A CSS mask is set. Masks are not rendered, so the element's background is not painted
    /// (an icon drawn with `background-color` + `mask-image` would otherwise be a solid block).
    pub masked: bool,
    pub content: Option<Rc<[ContentItem]>>,
    pub object_fit: ObjectFit,
    pub aspect_ratio: Option<f32>,
    pub translate: Option<(Size, Size)>,
    pub outline_width: f32,
    pub outline_style: BorderStyle,
    pub outline_color: Color,
    pub outline_offset: f32,
    pub table_layout_fixed: bool,
    /// Bit i set: border_color[i] was set explicitly (otherwise it follows `color`).
    pub border_color_set: u8,
    pub outline_color_set: bool,
}

thread_local! {
    static DEFAULT_FAMILY: Rc<[Atom]> = Rc::from(vec![Atom::new("serif")]);
    static INITIAL: Rc<ComputedStyle> = Rc::new(ComputedStyle::build_initial());
}

impl ComputedStyle {
    fn build_initial() -> ComputedStyle {
        ComputedStyle {
            color: Color::BLACK,
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            font_family: DEFAULT_FAMILY.with(|f| f.clone()),
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            visibility: Visibility::Visible,
            cursor: Cursor::Auto,
            list_style: ListStyle::Disc,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_transform: TextTransform::None,
            text_indent: 0.0,
            border_collapse: false,
            border_spacing: (0.0, 0.0),
            overflow_wrap: OverflowWrap::Normal,
            break_all: false,
            pointer_events: true,
            text_decoration: 0,
            vars: None,

            display: Display::Inline,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
            box_sizing: BoxSizing::ContentBox,
            overflow_x: Overflow::Visible,
            overflow_y: Overflow::Visible,
            width: Size::Auto,
            height: Size::Auto,
            min_width: Size::Auto,
            min_height: Size::Auto,
            max_width: Size::Auto,
            max_height: Size::Auto,
            margin: [Size::ZERO, Size::ZERO, Size::ZERO, Size::ZERO],
            padding: [Size::ZERO, Size::ZERO, Size::ZERO, Size::ZERO],
            inset: [Size::Auto, Size::Auto, Size::Auto, Size::Auto],
            border_width: [0.0; 4],
            border_style: [BorderStyle::None; 4],
            border_color: [Color::BLACK; 4],
            border_radius: [Size::ZERO, Size::ZERO, Size::ZERO, Size::ZERO],
            background_color: Color::TRANSPARENT,
            background: None,
            box_shadow: None,
            opacity: 1.0,
            z_index: None,
            vertical_align: VAlign::Baseline,
            text_decoration_own: 0,
            text_overflow_ellipsis: false,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Nowrap,
            justify_content: Align::Normal,
            align_items: Align::Normal,
            align_self: Align::Auto,
            align_content: Align::Normal,
            justify_items: Align::Normal,
            justify_self: Align::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Size::Auto,
            order: 0,
            row_gap: Size::ZERO,
            column_gap: Size::ZERO,
            grid_template_columns: None,
            grid_template_rows: None,
            grid_auto_rows: None,
            grid_auto_flow_column: false,
            grid_column: (GridLine::Auto, GridLine::Auto),
            grid_row: (GridLine::Auto, GridLine::Auto),
            grid_template_areas: None,
            grid_area: Atom::NONE,
            masked: false,
            bg_clip_text: false,
            text_bg: None,
            text_fill: None,
            content: None,
            object_fit: ObjectFit::Fill,
            aspect_ratio: None,
            translate: None,
            outline_width: 0.0,
            outline_style: BorderStyle::None,
            outline_color: Color::BLACK,
            outline_offset: 0.0,
            table_layout_fixed: false,
            border_color_set: 0,
            outline_color_set: false,
        }
    }

    pub fn initial() -> Rc<ComputedStyle> {
        INITIAL.with(|i| i.clone())
    }

    /// A fresh style for a child of `parent`: inherited properties copied, the rest initial.
    pub fn inherit_from(parent: &ComputedStyle) -> ComputedStyle {
        let init = Self::initial();
        ComputedStyle {
            color: parent.color,
            text_bg: parent.text_bg.clone(),
            text_fill: parent.text_fill,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            italic: parent.italic,
            font_family: parent.font_family.clone(),
            line_height: parent.line_height,
            text_align: parent.text_align,
            white_space: parent.white_space,
            visibility: parent.visibility,
            cursor: parent.cursor,
            list_style: parent.list_style,
            letter_spacing: parent.letter_spacing,
            word_spacing: parent.word_spacing,
            text_transform: parent.text_transform,
            text_indent: parent.text_indent,
            border_collapse: parent.border_collapse,
            border_spacing: parent.border_spacing,
            overflow_wrap: parent.overflow_wrap,
            break_all: parent.break_all,
            pointer_events: parent.pointer_events,
            text_decoration: parent.text_decoration,
            vars: parent.vars.clone(),
            ..(*init).clone()
        }
    }

    /// Used line height in pixels for text set in this style, given the font's natural height.
    pub fn line_height_px(&self, natural: f32) -> f32 {
        match self.line_height {
            LineHeight::Normal => natural,
            LineHeight::Number(n) => n * self.font_size,
            LineHeight::Px(p) => p,
        }
    }

    pub fn is_positioned(&self) -> bool {
        self.position != Position::Static
    }

    pub fn is_out_of_flow(&self) -> bool {
        matches!(self.position, Position::Absolute | Position::Fixed)
    }

    pub fn is_floating(&self) -> bool {
        self.float != Float::None && !self.is_out_of_flow()
    }

    pub fn clips(&self) -> bool {
        self.overflow_x != Overflow::Visible || self.overflow_y != Overflow::Visible
    }

    pub fn has_visible_border(&self) -> bool {
        self.border_width.iter().any(|w| *w > 0.0)
    }

    pub fn wraps(&self) -> bool {
        !matches!(self.white_space, WhiteSpace::Nowrap | WhiteSpace::Pre)
    }

    pub fn preserves_ws(&self) -> bool {
        matches!(self.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap)
    }

    pub fn preserves_newlines(&self) -> bool {
        matches!(self.white_space, WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine)
    }
}
