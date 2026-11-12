//! Design tokens, taken from the website's `variables.css`.

use invisium_draw::Rgba;

#[derive(Clone, Debug)]
pub struct Theme {
    // Backgrounds
    pub bg_deep: Rgba,
    pub bg_body: Rgba,
    pub bg_card: Rgba,
    pub bg_card_hover: Rgba,
    pub bg_elevated: Rgba,
    // Borders
    pub border: Rgba,
    pub border_light: Rgba,
    // Text
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    /// `text_muted` at reduced alpha, for disabled controls.
    pub text_disabled: Rgba,
    // Brand blues
    pub blue_400: Rgba,
    pub blue_500: Rgba,
    pub blue_600: Rgba,
    pub blue_700: Rgba,
    pub blue_glow: Rgba,
    // Semantic
    pub success: Rgba,
    pub danger: Rgba,
    pub warning: Rgba,
    /// The site's `rgba(255,255,255,0.05)` nav hover.
    pub hover_overlay: Rgba,
    pub white: Rgba,
    // Radii
    pub radius: f32,
    pub radius_sm: f32,
    pub radius_xs: f32,
    // Fonts
    pub font_ui: String,
    pub font_ui_fallback: String,
    pub font_mono: String,
    pub font_icons: String,
    // Metrics (logical px)
    pub tab_strip_h: f32,
    pub toolbar_h: f32,
    pub tab_h: f32,
    pub tab_max_w: f32,
    pub tab_gap: f32,
    pub button: f32,
    pub field_h: f32,
    pub progress_h: f32,
    pub text_size: f32,
    pub small_size: f32,
    pub icon_size: f32,
}

impl Theme {
    pub fn invis() -> Theme {
        Theme {
            bg_deep: Rgba::hex(0x0a0f1a),
            bg_body: Rgba::hex(0x111827),
            bg_card: Rgba::hex(0x1f2937),
            bg_card_hover: Rgba::hex(0x263348),
            bg_elevated: Rgba::hex(0x283548),
            border: Rgba::hex(0x374151),
            border_light: Rgba::new(59, 130, 246, 38),
            text_primary: Rgba::hex(0xf3f4f6),
            text_secondary: Rgba::hex(0xd1d5db),
            text_muted: Rgba::hex(0x9ca3af),
            text_disabled: Rgba::new(156, 163, 175, 110),
            blue_400: Rgba::hex(0x60a5fa),
            blue_500: Rgba::hex(0x3b82f6),
            blue_600: Rgba::hex(0x2563eb),
            blue_700: Rgba::hex(0x1e40af),
            blue_glow: Rgba::new(59, 130, 246, 64),
            success: Rgba::hex(0x10b981),
            danger: Rgba::hex(0xef4444),
            warning: Rgba::hex(0xf59e0b),
            hover_overlay: Rgba::new(255, 255, 255, 13),
            white: Rgba::WHITE,
            radius: 16.0,
            radius_sm: 10.0,
            radius_xs: 6.0,
            font_ui: "Geist".into(),
            font_ui_fallback: "Segoe UI".into(),
            font_mono: "Geist Mono".into(),
            font_icons: "Segoe MDL2 Assets".into(),
            tab_strip_h: 36.0,
            toolbar_h: 44.0,
            tab_h: 32.0,
            tab_max_w: 220.0,
            tab_gap: 2.0,
            button: 32.0,
            field_h: 32.0,
            progress_h: 2.0,
            text_size: 13.0,
            small_size: 12.0,
            icon_size: 14.0,
        }
    }

    /// Every colour the chrome may paint with.
    pub fn palette(&self) -> Vec<Rgba> {
        vec![
            self.bg_deep,
            self.bg_body,
            self.bg_card,
            self.bg_card_hover,
            self.bg_elevated,
            self.border,
            self.border_light,
            self.text_primary,
            self.text_secondary,
            self.text_muted,
            self.text_disabled,
            self.blue_400,
            self.blue_500,
            self.blue_600,
            self.blue_700,
            self.blue_glow,
            self.success,
            self.danger,
            self.warning,
            self.hover_overlay,
            self.white,
        ]
    }
}
